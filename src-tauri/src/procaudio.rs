//! The current game's own sound, for the optional "Game" audio track.
//!
//! Windows 10 version 2004 (build 19041) and newer can capture what one
//! process tree plays ("process loopback"): `ActivateAudioInterfaceAsync` on the
//! virtual device `VAD\Process_Loopback` with the game's process id gives an
//! `IAudioClient` that delivers only that tree's audio, in the format we ask for
//! (48 kHz stereo s16 here). It goes into a third `AudioRing`, beside desktop
//! and microphone, anchored to the same app clock, so a clip cuts it to the
//! same wall-clock ranges as the video.
//!
//! Like desktop loopback, it sends nothing while the process is silent; the
//! capture thread then pads the ring with silence (`AudioRing::pad_to`) so it
//! keeps up with the clock. When a game ends the capture stops but the ring is
//! kept: a clip taken a moment later still has the end of the game in it.
//! `spans` remembers when a capture was actually running, so a clip that does
//! not overlap any of that time gets no Game track at all.
use crate::audio::{AudioRing, Plan};
use crate::util::{self, now_secs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use windows::core::{implement, Interface, HRESULT, IUnknown};
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

const RATE: u32 = 48_000;
/// First Windows build with process loopback (Windows 10 version 2004).
const MIN_BUILD: u32 = 19041;
/// `AUDCLNT_BUFFERFLAGS_SILENT`: the packet is silence, whatever the buffer holds.
const BUFFERFLAGS_SILENT: u32 = 2;
/// `VT_BLOB`.
const VT_BLOB: u16 = 65;

/// Where the ring keeps its audio; a change reconfigures it.
#[derive(Clone, PartialEq, Debug)]
pub struct RingCfg {
    /// Seconds the ring holds (the longest buffer plus a margin).
    pub max_secs: f64,
    /// See `spill::window`.
    pub window: Option<f64>,
    pub dir: PathBuf,
}

struct Run {
    pid: u32,
    stop: Arc<AtomicBool>,
}

struct State {
    run: Option<Run>,
    cfg: Option<RingCfg>,
}

/// What the Capture page shows: "off", "waiting" (on, no game), "capturing",
/// "failed" (this game's sound could not be captured), "unavailable" (Windows too old).
#[derive(Default)]
struct Shared {
    status: Mutex<String>,
    /// Wall-clock spans in which a capture was running: (run id, from, to).
    spans: Mutex<Vec<(u64, f64, f64)>>,
    /// Frames the API delivered (not counting padding), for the self-test.
    delivered: AtomicU64,
    /// Samples above silence among them.
    loud: AtomicU64,
}

pub struct GameAudio {
    ring: Arc<Mutex<AudioRing>>,
    state: Mutex<State>,
    shared: Arc<Shared>,
}

/// The app's one game-audio capture.
pub fn global() -> &'static GameAudio {
    static G: OnceLock<GameAudio> = OnceLock::new();
    G.get_or_init(GameAudio::new)
}

/// Windows' build number (0 if it cannot be read).
pub fn windows_build() -> u32 {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// True if this Windows can capture one process's sound.
pub fn supported() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    *S.get_or_init(|| {
        let b = windows_build();
        // 0 = unknown: try, and let activation decide.
        b == 0 || b >= MIN_BUILD
    })
}

impl GameAudio {
    pub fn new() -> Self {
        GameAudio {
            ring: Arc::new(Mutex::new(AudioRing::new())),
            state: Mutex::new(State { run: None, cfg: None }),
            shared: Arc::new(Shared { status: Mutex::new("off".into()), ..Default::default() }),
        }
    }

    pub fn status(&self) -> String {
        self.shared.status.lock().unwrap().clone()
    }

    fn set_status(&self, s: &str) {
        *self.shared.status.lock().unwrap() = s.to_string();
    }

    /// Bytes held in RAM and in files.
    pub fn bytes(&self) -> (usize, usize) {
        self.ring.lock().unwrap().bytes()
    }

    /// The setting is off or clipping is paused: stop, and drop what was captured.
    pub fn off(&self) {
        let mut st = self.state.lock().unwrap();
        if let Some(r) = st.run.take() {
            r.stop.store(true, Ordering::Relaxed);
        }
        st.cfg = None;
        self.ring.lock().unwrap().clear();
        self.shared.spans.lock().unwrap().clear();
        drop(st);
        self.set_status("off");
    }

    /// On, with `pid` the current game (None = no game). Starts, stops or
    /// switches the capture as needed; nothing runs without a game.
    pub fn sync(&self, pid: Option<u32>, cfg: RingCfg) {
        let mut st = self.state.lock().unwrap();
        if !supported() {
            if let Some(r) = st.run.take() {
                r.stop.store(true, Ordering::Relaxed);
            }
            drop(st);
            if self.status() != "unavailable" {
                util::log(&format!("game audio: not available on Windows build {} (needs {MIN_BUILD} or newer)", windows_build()));
                self.set_status("unavailable");
            }
            return;
        }
        let horizon = now_secs() - cfg.max_secs - 5.0;
        self.shared.spans.lock().unwrap().retain(|s| s.2 >= horizon);
        if st.cfg.as_ref() != Some(&cfg) {
            let mut ring = self.ring.lock().unwrap();
            ring.configure(RATE, cfg.max_secs);
            ring.set_spill(cfg.window, cfg.dir.clone(), "g");
            st.cfg = Some(cfg.clone());
        }
        match pid {
            None => {
                if let Some(r) = st.run.take() {
                    r.stop.store(true, Ordering::Relaxed);
                }
                drop(st);
                self.set_status("waiting");
            }
            // Already started for this game. A thread that ended by itself (the
            // process exited, or it refused to be captured) is not restarted:
            // the game watch would otherwise retry a refusing process every 2 s.
            Some(p) if st.run.as_ref().is_some_and(|r| r.pid == p) => {}
            Some(p) => {
                if let Some(r) = st.run.take() {
                    r.stop.store(true, Ordering::Relaxed);
                }
                let stop = Arc::new(AtomicBool::new(false));
                st.run = Some(Run { pid: p, stop: stop.clone() });
                drop(st);
                self.spawn(p, stop);
            }
        }
    }

    /// The process the capture is for, while its thread runs.
    pub fn running_pid(&self) -> Option<u32> {
        self.state.lock().unwrap().run.as_ref().filter(|r| !r.stop.load(Ordering::Relaxed)).map(|r| r.pid)
    }

    fn spawn(&self, pid: u32, stop: Arc<AtomicBool>) {
        static IDS: AtomicU64 = AtomicU64::new(1);
        let id = IDS.fetch_add(1, Ordering::Relaxed);
        let ring = self.ring.clone();
        let shared = self.shared.clone();
        let started = thread::Builder::new().name("game-audio".into()).spawn(move || {
            let result = capture(pid, &ring, &stop, &shared, id);
            // Ended by itself or asked to: either way `sync` sees it as not running.
            let asked = stop.swap(true, Ordering::Relaxed);
            match result {
                Err(e) if !asked => {
                    util::log(&format!("game audio: process {pid}: {e}"));
                    *shared.status.lock().unwrap() = "failed".into();
                }
                Ok(()) if !asked => *shared.status.lock().unwrap() = "waiting".into(),
                _ => {}
            }
        });
        if started.is_err() {
            self.set_status("failed");
        }
    }

    /// The game's sound for a list of wall-clock ranges (the clip's video
    /// segments), or None when no capture ran during any of them.
    pub fn plan(&self, segs: &[(f64, f64)]) -> Option<Plan> {
        let hit = {
            let spans = self.shared.spans.lock().unwrap();
            spans.iter().any(|&(_, a, b)| segs.iter().any(|&(x, y)| x < b && y > a))
        };
        if !hit {
            return None;
        }
        self.ring.lock().unwrap().plan(segs)
    }

    /// Test hook: put made-up game sound into the ring as if it had been
    /// captured from `t0` on (stereo s16 at 48 kHz).
    pub fn feed(&self, stereo: &[i16], t0: f64) {
        let n = stereo.len() / 2;
        let t1 = t0 + n as f64 / RATE as f64;
        let mut ring = self.ring.lock().unwrap();
        // In 10 ms packets, as the API delivers them.
        let step = (RATE / 100) as usize * 2;
        let mut at = 0usize;
        while at < stereo.len() {
            let end = (at + step).min(stereo.len());
            ring.push(&stereo[at..end], t0 + (end / 2) as f64 / RATE as f64);
            at = end;
        }
        drop(ring);
        self.shared.spans.lock().unwrap().push((0, t0, t1));
    }
}

#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Done(Arc<(Mutex<bool>, Condvar)>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
    fn ActivateCompleted(&self, _op: Option<&IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        let (lock, cv) = &*self.0;
        *lock.lock().unwrap() = true;
        cv.notify_all();
        Ok(())
    }
}

/// PROPVARIANT holding a VT_BLOB, laid out as the real one (24 bytes on x64).
/// Built by hand so nothing ever tries to free the blob, which lives on our stack.
#[repr(C)]
struct BlobVariant {
    vt: u16,
    r1: u16,
    r2: u16,
    r3: u16,
    size: u32,
    data: *const u8,
}

/// An `IAudioClient` for `pid`'s process tree.
fn activate(pid: u32) -> Result<IAudioClient, String> {
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS { TargetProcessId: pid, ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE },
        },
    };
    let pv = BlobVariant { vt: VT_BLOB, r1: 0, r2: 0, r3: 0, size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32, data: &params as *const _ as *const u8 };
    let signal = Arc::new((Mutex::new(false), Condvar::new()));
    let handler: IActivateAudioInterfaceCompletionHandler = Done(signal.clone()).into();
    unsafe {
        let op = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&pv as *const BlobVariant as *const windows::core::PROPVARIANT), &handler)
            .map_err(|e| format!("activation refused ({e})"))?;
        let (lock, cv) = &*signal;
        let guard = lock.lock().unwrap();
        let (guard, timeout) = cv.wait_timeout_while(guard, Duration::from_secs(5), |done| !*done).unwrap();
        drop(guard);
        if timeout.timed_out() {
            return Err("activation did not finish within 5 s".into());
        }
        let mut hr = HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| format!("no activation result ({e})"))?;
        hr.ok().map_err(|e| format!("activation failed ({e})"))?;
        unk.ok_or("activation gave nothing")?.cast::<IAudioClient>().map_err(|e| format!("not an audio client ({e})"))
    }
}

/// Activate, then read packets until `stop`, the process exits, or the stream
/// breaks (re-activated after a pause: a device change ends the stream).
/// Err only when the very first activation fails.
fn capture(pid: u32, ring: &Mutex<AudioRing>, stop: &AtomicBool, shared: &Shared, id: u64) -> Result<(), String> {
    struct Com;
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let _com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok().then_some(Com);
    let fmt = WAVEFORMATEX { wFormatTag: 1, nChannels: 2, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * 4, nBlockAlign: 4, wBitsPerSample: 16, cbSize: 0 };
    let mut first = true;
    let mut buf: Vec<i16> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let client = match activate(pid) {
            Ok(c) => c,
            Err(e) if first => return Err(e),
            Err(e) => {
                util::log(&format!("game audio: process {pid}: {e}; trying again"));
                if !wait(stop, 2000) || !crate::win::process_alive(pid) {
                    return Ok(());
                }
                continue;
            }
        };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        // A one-second buffer: polled every 10 ms, it never overflows.
        let cap: IAudioCaptureClient = match unsafe { client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, &fmt, None).and_then(|_| client.GetService::<IAudioCaptureClient>()).and_then(|c| client.Start().map(|_| c)) } {
            Ok(c) => c,
            Err(e) if first => return Err(format!("stream did not start ({e})")),
            Err(e) => {
                util::log(&format!("game audio: process {pid}: stream did not start ({e}); trying again"));
                if !wait(stop, 2000) {
                    return Ok(());
                }
                continue;
            }
        };
        if first {
            util::log(&format!("game audio: capturing process {pid}"));
        }
        first = false;
        *shared.status.lock().unwrap() = "capturing".into();
        let now = now_secs();
        ring.lock().unwrap().pad_to(now);
        shared.spans.lock().unwrap().push((id, now, now));
        let mut last_alive = std::time::Instant::now();
        let mut broken = false;
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(10));
            buf.clear();
            loop {
                let n = match unsafe { cap.GetNextPacketSize() } {
                    Ok(n) => n,
                    Err(e) => {
                        util::log(&format!("game audio: process {pid}: stream ended ({e}), restarting"));
                        broken = true;
                        break;
                    }
                };
                if n == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut bflags = 0u32;
                if let Err(e) = unsafe { cap.GetBuffer(&mut data, &mut frames, &mut bflags, None, None) } {
                    util::log(&format!("game audio: process {pid}: read failed ({e}), restarting"));
                    broken = true;
                    break;
                }
                let samples = frames as usize * 2;
                if bflags & BUFFERFLAGS_SILENT != 0 || data.is_null() {
                    buf.extend(std::iter::repeat(0i16).take(samples));
                } else {
                    let s = unsafe { std::slice::from_raw_parts(data as *const i16, samples) };
                    shared.loud.fetch_add(s.iter().filter(|v| v.unsigned_abs() > 32).count() as u64, Ordering::Relaxed);
                    buf.extend_from_slice(s);
                }
                shared.delivered.fetch_add(frames as u64, Ordering::Relaxed);
                let _ = unsafe { cap.ReleaseBuffer(frames) };
            }
            let now = now_secs();
            {
                let mut r = ring.lock().unwrap();
                if buf.is_empty() {
                    r.pad_to(now);
                } else {
                    r.push(&buf, now);
                }
            }
            if let Some(s) = shared.spans.lock().unwrap().iter_mut().rev().find(|s| s.0 == id) {
                s.2 = now;
            }
            if broken {
                break;
            }
            if last_alive.elapsed() >= Duration::from_secs(1) {
                last_alive = std::time::Instant::now();
                if !crate::win::process_alive(pid) {
                    util::log(&format!("game audio: process {pid} ended"));
                    let _ = unsafe { client.Stop() };
                    return Ok(());
                }
            }
        }
        let _ = unsafe { client.Stop() };
        if broken && !wait(stop, 500) {
            return Ok(());
        }
    }
    Ok(())
}

/// Sleep up to `ms`, waking early on `stop`. False if stopped.
fn wait(stop: &AtomicBool, ms: u64) -> bool {
    let mut left = ms;
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(50);
        thread::sleep(Duration::from_millis(step));
        left -= step;
    }
    !stop.load(Ordering::Relaxed)
}

/* ---------- self-test ---------- */

struct Report {
    path: PathBuf,
    passed: u32,
    failed: u32,
}

impl Report {
    fn line(&self, s: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{s}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        self.line(&format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!(" ({detail})") }));
    }
}

/// `glint.exe --gameaudio-test <report.txt>`: the Game track from ring to clip,
/// through the editor, trim and compress, old layouts unchanged; then a live
/// process-loopback capture that makes no sound (it listens to a process that
/// is already playing, or to a silent one).
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), passed: 0, failed: 0 };
    let _ = std::fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    test_ring(&mut r);
    test_clips(&mut r);
    test_live(&mut r);
    let (p, f) = (r.passed, r.failed);
    r.line(&format!("DONE passed={p} failed={f}"));
}

/// Interleaved stereo sine, `amp` of full scale.
fn tone(freq: f64, secs: f64, amp: f64) -> Vec<i16> {
    let n = (secs * RATE as f64) as usize;
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = ((i as f64 / RATE as f64) * freq * std::f64::consts::TAU).sin() * amp * 32767.0;
        v.push(s as i16);
        v.push(s as i16);
    }
    v
}

fn test_ring(r: &mut Report) {
    // Packets, then nothing: the ring still reaches the present.
    let mut ring = AudioRing::new();
    ring.configure(RATE, 60.0);
    let t = tone(500.0, 0.5, 0.3);
    for (k, p) in t.chunks(960).enumerate() {
        ring.push(p, 10.0 + (k + 1) as f64 * 0.01);
    }
    let before = ring.bytes().0 / 4;
    ring.pad_to(10.05 + 0.5); // within the 0.1 s tolerance: nothing added
    let same = ring.bytes().0 / 4 == before;
    ring.pad_to(13.0);
    let frames = ring.bytes().0 / 4;
    r.check("silence: the ring advances with the clock", (143_000..=145_000).contains(&frames), &format!("{frames} frames for 3.0 s"));
    r.check("silence: a short pause adds nothing", same, &format!("{before} frames"));
    let got = ring.slices(&[(10.0, 13.0)]).map(|p| p.data).unwrap_or_default();
    let loud_head = got[..48_000].iter().filter(|v| v.unsigned_abs() > 1000).count();
    let tail_silent = got[60_000..].iter().all(|v| *v == 0);
    r.check("silence: sound stays at its time, the rest is silent", got.len() == 288_000 && loud_head > 10_000 && tail_silent, &format!("{} samples, {loud_head} loud in the first 0.5 s", got.len()));
    let mut fresh = AudioRing::new();
    fresh.configure(RATE, 60.0);
    fresh.pad_to(5.0);
    fresh.pad_to(6.0);
    let n = fresh.bytes().0 / 4;
    r.check("silence: a ring with no packet at all still keeps time", (47_000..=49_000).contains(&n), &format!("{n} frames for 1.0 s"));

    // The game ring: no Game track unless a capture ran during the clip.
    let dir = util::data_dir().join("gameaudio-test").join("ring");
    let _ = std::fs::create_dir_all(&dir);
    let ga = GameAudio::new();
    ga.sync(None, RingCfg { max_secs: 1e9, window: Some(0.0), dir: dir.clone() });
    r.check("no game: status waiting, nothing running", ga.status() == "waiting" && ga.running_pid().is_none(), &ga.status());
    r.check("empty game ring gives no track", ga.plan(&[(100.0, 104.0)]).is_none(), "");
    ga.feed(&tone(500.0, 2.0, 0.3), 101.0);
    let p = ga.plan(&[(100.0, 104.0)]).map(Plan::read);
    let ok = p.as_ref().is_some_and(|p| {
        let d = &p.data;
        d.len() == 4 * 96_000 && d[..96_000].iter().all(|v| *v == 0) && d[96_000..288_000].iter().filter(|v| v.unsigned_abs() > 1000).count() > 100_000 && d[290_000..].iter().all(|v| *v == 0)
    });
    r.check("game ring: a clip over the game gets it, silence where there was none", ok, &format!("{:?} samples", p.as_ref().map(|p| p.data.len())));
    r.check("game ring: a clip outside the game's time gets no track", ga.plan(&[(200.0, 204.0)]).is_none(), "");
    let first = p.map(|p| p.data).unwrap_or_default();
    ga.feed(&tone(700.0, 12.0, 0.3), 110.0);
    let again = ga.plan(&[(100.0, 104.0)]).map(Plan::read).map(|p| p.data).unwrap_or_default();
    r.check("game ring: older audio goes to the buffer folder and reads back the same", ga.bytes().1 > 0 && !first.is_empty() && again == first, &format!("{:?}", ga.bytes()));
    ga.off();
    r.check("off: the ring and its time are dropped", ga.plan(&[(100.0, 104.0)]).is_none() && ga.bytes() == (0, 0) && ga.status() == "off", "");
}

use crate::clip::audio_titles as titles;

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
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse().ok()))
        .unwrap_or(-999.0)
}

fn test_clips(r: &mut Report) {
    use crate::clip;
    use crate::editor::{self, AudioEdit, ExportOpts};
    use crate::library;
    use crate::recorder::Snapshot;
    use std::path::Path;
    let dir = util::data_dir().join("gameaudio-test").join("clips");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    // The video part of a snapshot: 4 s of MPEG-TS, as the capture pipe gives it.
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
    let s = crate::settings::Settings { clips_dir: dir.to_string_lossy().into_owned(), game_folders: false, mic_denoise: false, ..Default::default() };
    let pcm = |v: Vec<i16>| Some(crate::audio::Pcm { rate: RATE, data: v });
    // Desktop 1000 Hz, microphone 3000 Hz, game 500 Hz (only 1..3 s: the rest of the clip is silence).
    let ga = GameAudio::new();
    ga.sync(None, RingCfg { max_secs: 1e9, window: None, dir: dir.clone() });
    ga.feed(&tone(500.0, 2.0, 0.3), 101.0);
    let segs = [(100.0, 104.0)];
    let empty = GameAudio::new();
    let write = |desk: bool, mic: bool, game: Option<&GameAudio>, name: &str| {
        let cap = clip::Capture {
            snap: Snapshot { header: header.clone(), gops: Vec::new(), segments: segs.to_vec(), duration: 4.0 },
            desk: if desk { pcm(tone(1000.0, 4.0, 0.3)) } else { None },
            mic: if mic { pcm(tone(3000.0, 4.0, 0.3)) } else { None },
            game: game.and_then(|g| g.plan(&segs)).map(Plan::read),
            width: 320,
            height: 180,
            fps: 30,
        };
        clip::write(&s, cap, name)
    };
    let mut clips: Vec<(&str, library::ClipMeta)> = Vec::new();
    for (label, desk, mic, game, want) in [
        ("desktop + mic + game", true, true, Some(&ga), vec!["Mix", "Desktop", "Microphone", "Game"]),
        ("desktop + game", true, false, Some(&ga), vec!["Desktop", "Game"]),
        ("desktop + mic, game ring empty", true, true, Some(&empty), vec!["Mix", "Desktop", "Microphone"]),
        ("desktop only", true, false, None, vec!["Desktop"]),
    ] {
        match write(desk, mic, game, label) {
            Ok(m) => {
                let got = titles(&ffmpeg, Path::new(&m.path));
                r.check(&format!("clip {label}: tracks {want:?}"), got == want && m.audio_tracks as usize == want.len(), &format!("{got:?}, audio_tracks {}", m.audio_tracks));
                clips.push((label, m));
            }
            Err(e) => r.check(&format!("clip {label}"), false, &e),
        }
    }
    let Some(four) = clips.iter().find(|c| c.0 == "desktop + mic + game").map(|c| c.1.clone()) else { return };
    let two = clips.iter().find(|c| c.0 == "desktop + game").map(|c| c.1.clone());
    let three = clips.iter().find(|c| c.0 == "desktop + mic, game ring empty").map(|c| c.1.clone());
    let one = clips.iter().find(|c| c.0 == "desktop only").map(|c| c.1.clone());
    let fp = Path::new(&four.path);
    let (g500, g1000) = (band(&ffmpeg, fp, 3, 500), band(&ffmpeg, fp, 3, 1000));
    r.check("4-track: the Game track holds the game", g500 - g1000 > 20.0, &format!("500 Hz {g500:.1} dB, 1000 Hz {g1000:.1} dB"));
    let (m500, m1000, m3000) = (band(&ffmpeg, fp, 0, 500), band(&ffmpeg, fp, 0, 1000), band(&ffmpeg, fp, 0, 3000));
    r.check("4-track: the Mix is desktop + mic, without the Game track", m1000 - m500 > 20.0 && m3000 - m500 > 20.0, &format!("500 {m500:.1}, 1000 {m1000:.1}, 3000 {m3000:.1} dB"));

    // Editor: rows come from the stream titles.
    for (label, clip, want) in [
        ("4-track", Some(four.clone()), vec!["Mix", "Desktop", "Microphone", "Game"]),
        ("2-track", two.clone(), vec!["Desktop", "Game"]),
        ("old 3-track", three.clone(), vec!["Mix", "Desktop", "Microphone"]),
        ("old 1-track", one.clone(), vec!["Desktop"]),
    ] {
        let Some(c) = clip else { continue };
        match editor::audio_tracks(&s, &c) {
            Ok(t) => {
                let got: Vec<&str> = t.iter().map(|x| x.title.as_str()).collect();
                r.check(&format!("editor {label}: track titles"), got == want, &format!("{got:?}"));
            }
            Err(e) => r.check(&format!("editor {label}: track titles"), false, &e),
        }
    }
    let ex = |c: &library::ClipMeta, opts: ExportOpts| editor::export(&s, c, &opts, &mut |_| {});
    for (label, clip, n) in [("4-track", Some(four.clone()), 4usize), ("2-track", two.clone(), 2)] {
        let Some(c) = clip else { continue };
        match ex(&c, ExportOpts { speed: 2.0, ..Default::default() }) {
            Ok(m) => {
                let got = titles(&ffmpeg, Path::new(&m.path));
                r.check(&format!("editor {label}: speed 2 keeps every track and title"), got.len() == n && got == titles(&ffmpeg, Path::new(&c.path)), &format!("{got:?}"));
            }
            Err(e) => r.check(&format!("editor {label}: speed 2"), false, &e),
        }
        match ex(&c, ExportOpts { start: 1.0, end: 3.0, ..Default::default() }) {
            Ok(m) => {
                let got = titles(&ffmpeg, Path::new(&m.path));
                r.check(&format!("editor {label}: lossless cut keeps every track and title"), got.len() == n && got == titles(&ffmpeg, Path::new(&c.path)) && m.kind == "trim", &format!("{} {got:?}", m.kind));
            }
            Err(e) => r.check(&format!("editor {label}: lossless cut"), false, &e),
        }
    }
    // Desktop muted, Game kept: the result has the game and the mic, not the rest of the desktop.
    let row = |index: u32| AudioEdit { source: "track".into(), index, volume: 1.0, ..Default::default() };
    match ex(&four, ExportOpts { audio: vec![row(2), row(3)], ..Default::default() }) {
        Ok(m) => {
            let p = Path::new(&m.path);
            let (a, b, c) = (band(&ffmpeg, p, 0, 500), band(&ffmpeg, p, 0, 1000), band(&ffmpeg, p, 0, 3000));
            r.check("editor 4-track: Game instead of Desktop mixes game + mic", titles(&ffmpeg, p).len() == 1 && a - b > 20.0 && c - b > 20.0, &format!("500 {a:.1}, 1000 {b:.1}, 3000 {c:.1} dB"));
        }
        Err(e) => r.check("editor 4-track: Game instead of Desktop", false, &e),
    }
    if let Some(two) = &two {
        match ex(two, ExportOpts { audio: vec![row(1)], ..Default::default() }) {
            Ok(m) => {
                let p = Path::new(&m.path);
                let (a, b) = (band(&ffmpeg, p, 0, 500), band(&ffmpeg, p, 0, 1000));
                r.check("editor 2-track: Game alone", titles(&ffmpeg, p) == ["Game"] && a - b > 20.0, &format!("{:?}, 500 {a:.1}, 1000 {b:.1} dB", titles(&ffmpeg, p)));
            }
            Err(e) => r.check("editor 2-track: Game alone", false, &e),
        }
    }

    // Trim without the microphone.
    for (label, clip, want) in [("4-track", Some(four.clone()), vec!["Desktop", "Game"]), ("2-track", two.clone(), vec!["Desktop", "Game"]), ("old 3-track", three.clone(), vec!["Desktop"])] {
        let Some(c) = clip else { continue };
        match clip::trim(&s, &c, 0.0, 4.0, false) {
            Ok(m) => {
                let got = titles(&ffmpeg, Path::new(&m.path));
                r.check(&format!("trim without mic {label}"), got == want, &format!("{got:?}"));
            }
            Err(e) => r.check(&format!("trim without mic {label}"), false, &e),
        }
    }

    // Compress for Discord keeps the first track: the Mix, or Desktop without a mic.
    for (label, clip, has_mic) in [("4-track", Some(four.clone()), true), ("2-track", two.clone(), false), ("old 1-track", one.clone(), false)] {
        let Some(c) = clip else { continue };
        let tag = format!("gameaudio-compress-{}", label.replace(' ', "-"));
        match crate::compress::compress(&ffmpeg, Path::new(&c.path), 10, &tag, "ultrafast", &AtomicBool::new(false), &mut |_| {}) {
            Ok(d) => {
                let (a, b, m) = (band(&ffmpeg, &d.file, 0, 500), band(&ffmpeg, &d.file, 0, 1000), band(&ffmpeg, &d.file, 0, 3000));
                let mic_ok = if has_mic { m - a > 20.0 } else { b - m > 20.0 };
                r.check(&format!("compress {label}: one track, the mix without the Game track"), titles(&ffmpeg, &d.file).len() == 1 && b - a > 20.0 && mic_ok, &format!("500 {a:.1}, 1000 {b:.1}, 3000 {m:.1} dB"));
                let _ = std::fs::remove_file(&d.file);
            }
            Err(e) => r.check(&format!("compress {label}"), false, &e),
        }
    }
    // Blur keeps the tracks but not their titles: they are put back (only for clips with a Game track).
    for (label, clip, want) in [("4-track", Some(four.clone()), 4usize), ("old 3-track", three.clone(), 0)] {
        let Some(c) = clip else { continue };
        let plain = dir.join(format!("untitled {label}.mp4"));
        let copied = util::hidden_cmd(&ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(&c.path).args(["-map", "0", "-c", "copy"]).arg(&plain).stdin(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        let before = titles(&ffmpeg, &plain);
        clip::restore_titles(&ffmpeg, Path::new(&c.path), &plain);
        let after = titles(&ffmpeg, &plain);
        let ok = if want > 0 { after == titles(&ffmpeg, Path::new(&c.path)) && after.len() == want } else { after == before };
        r.check(&format!("titles put back after Blur: {label}{}", if want > 0 { "" } else { " left as it was" }), copied && ok, &format!("{before:?} -> {after:?}"));
    }
    // Kept for a manual Blur run (`--blur-test`) on the new layouts.
    let keep = util::data_dir().join("gameaudio-test");
    let _ = std::fs::copy(&four.path, keep.join("4-track.mp4"));
    if let Some(two) = &two {
        let _ = std::fs::copy(&two.path, keep.join("2-track.mp4"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// File name of a process's executable.
fn exe_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        ok.then(|| path.rsplit('\\').next().unwrap_or("").to_string())
    }
}

/// Sessions on the default output device: (pid, peak level over about a second).
fn playing_sessions() -> Result<Vec<(u32, f32)>, String> {
    use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
    use windows::Win32::Media::Audio::{eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| e.to_string())?;
        let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
        let list = mgr.GetSessionEnumerator().map_err(|e| e.to_string())?;
        let mut meters: Vec<(u32, IAudioMeterInformation)> = Vec::new();
        for i in 0..list.GetCount().map_err(|e| e.to_string())? {
            let Ok(s) = list.GetSession(i) else { continue };
            let (Ok(s2), Ok(m)) = (s.cast::<IAudioSessionControl2>(), s.cast::<IAudioMeterInformation>()) else { continue };
            let pid = s2.GetProcessId().unwrap_or(0);
            if pid != 0 && pid != std::process::id() {
                meters.push((pid, m));
            }
        }
        let mut peaks: Vec<(u32, f32)> = meters.iter().map(|(p, _)| (*p, 0.0)).collect();
        for _ in 0..10 {
            for (i, (_, m)) in meters.iter().enumerate() {
                peaks[i].1 = peaks[i].1.max(m.GetPeakValue().unwrap_or(0.0));
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(peaks)
    }
}

fn test_live(r: &mut Report) {
    let build = windows_build();
    r.line(&format!("INFO Windows build {build}"));
    if !supported() {
        r.line("INFO process loopback needs build 19041 or newer: live test skipped");
        let ga = GameAudio::new();
        ga.sync(Some(4), RingCfg { max_secs: 60.0, window: None, dir: util::tmp_dir() });
        r.check("old Windows: the feature stays off with a status note", ga.status() == "unavailable" && ga.running_pid().is_none(), &ga.status());
        return;
    }
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let sessions = playing_sessions();
    if com {
        unsafe { CoUninitialize() };
    }
    let sessions = match sessions {
        Ok(s) => s,
        Err(e) => {
            r.line(&format!("INFO audio sessions could not be listed: {e}"));
            Vec::new()
        }
    };
    let name = |pid: u32| exe_name(pid).unwrap_or_else(|| "?".into());
    for (pid, peak) in &sessions {
        r.line(&format!("INFO session pid {pid} ({}) peak {peak:.3}", name(*pid)));
    }
    // Not a running Glint: its loopback capture shows as a session with a level,
    // but that process plays nothing (seen twice: 0 samples above silence).
    let loud = sessions.iter().filter(|s| s.1 > 0.001 && !name(s.0).eq_ignore_ascii_case("glint.exe")).max_by(|a, b| a.1.total_cmp(&b.1)).copied();
    let (pid, playing) = match loud {
        Some((p, _)) => (p, true),
        None => (std::process::id(), false),
    };
    r.line(&format!(
        "INFO live test on {}",
        if playing { format!("pid {pid} ({}), which is playing sound now", name(pid)) } else { format!("Glint's own process {pid}, which is silent (nothing was playing)") }
    ));
    let ga = GameAudio::new();
    let cfg = RingCfg { max_secs: 60.0, window: None, dir: util::tmp_dir() };
    ga.sync(Some(pid), cfg.clone());
    // Activation takes a moment; then measure over 4 s.
    let t0 = std::time::Instant::now();
    while ga.status() != "capturing" && ga.status() != "failed" && t0.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(20));
    }
    r.check("live: process loopback activates", ga.status() == "capturing", &format!("status {} after {} ms", ga.status(), t0.elapsed().as_millis()));
    if ga.status() != "capturing" {
        ga.off();
        return;
    }
    let (d0, f0, at0) = (ga.shared.delivered.load(Ordering::Relaxed), ga.bytes().0 / 4, now_secs());
    thread::sleep(Duration::from_secs(4));
    let (d1, f1, at1) = (ga.shared.delivered.load(Ordering::Relaxed), ga.bytes().0 / 4, now_secs());
    let secs = at1 - at0;
    let ring_rate = (f1 - f0) as f64 / secs;
    let api_rate = (d1 - d0) as f64 / secs;
    let loud_samples = ga.shared.loud.load(Ordering::Relaxed);
    r.check("live: the ring keeps time at about 48000 frames/s", (ring_rate - 48_000.0).abs() < 2_400.0, &format!("{ring_rate:.0} frames/s"));
    if playing {
        r.check("live: the API delivers about 48000 frames/s", (api_rate - 48_000.0).abs() < 4_800.0, &format!("{api_rate:.0} frames/s"));
        r.check("live: the samples are not silent", loud_samples > 1000, &format!("{loud_samples} samples above silence"));
    } else {
        r.line(&format!("INFO API delivered {api_rate:.0} frames/s from a silent process ({loud_samples} samples above silence)"));
    }
    let segs = [(at0, at1)];
    r.check("live: a clip over that time gets a Game track", ga.plan(&segs).map(Plan::read).is_some_and(|p| p.data.len() >= ((secs - 0.2) * 96_000.0) as usize), "");
    ga.sync(None, cfg.clone());
    thread::sleep(Duration::from_millis(300));
    r.check("live: no game stops the capture and keeps the audio", ga.running_pid().is_none() && ga.status() == "waiting" && ga.plan(&segs).is_some(), &ga.status());
    ga.off();

    // A process that exits by itself: the capture ends on its own.
    let ping = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("PING.EXE");
    match util::hidden_cmd(&ping).args(["-n", "3", "127.0.0.1"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null()).spawn() {
        Ok(mut child) => {
            let ga = GameAudio::new();
            ga.sync(Some(child.id()), cfg.clone());
            let _ = child.wait();
            let t0 = std::time::Instant::now();
            while ga.running_pid().is_some() && t0.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(50));
            }
            r.check("live: the capture ends when its process exits", ga.running_pid().is_none() && ga.status() == "waiting", &format!("status {} {} ms after exit", ga.status(), t0.elapsed().as_millis()));
            ga.off();
        }
        Err(e) => r.line(&format!("INFO could not start a short process: {e}")),
    }
}
