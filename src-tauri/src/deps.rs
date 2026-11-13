//! "Check and repair": find the programs Glint depends on, and install the
//! missing ones from the same places GlintSetup.exe uses (see setup.rs):
//!   - ffmpeg and ffprobe   github.com/GyanD/codexffmpeg, latest "essentials" build
//!   - Blur (optional)      github.com/f0e/blur v2.23, an Inno Setup installer
//! curl.exe and tar.exe (part of Windows 10 1803 and newer) do the download and the unpacking.
//!
//! The texts in `Item::note` and in the errors are fixed English sentences: the
//! UI translates them (ui/lang/*-deps.js). Details go to glint.log.
use crate::core::Core;
use crate::settings::Settings;
use crate::{blur, util};
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const FFMPEG_REPO: &str = "GyanD/codexffmpeg";
const BLUR_REPO: &str = "f0e/blur";
const BLUR_TAG: &str = "v2.23";
const BLUR_ASSET: &str = "blur-Windows-Installer-x64.exe";

// Errors the UI shows (and translates).
pub const E_OFFLINE: &str = "GitHub could not be reached. Check your internet connection and try again.";
pub const E_DOWNLOAD: &str = "The download failed. Check your internet connection and try again.";
pub const E_CANCELLED: &str = "Cancelled.";
pub const E_UNPACK: &str = "The download could not be unpacked.";
pub const E_NO_RUN: &str = "The downloaded ffmpeg does not run on this PC.";
pub const E_PUT: &str = "Glint could not put the files in place. Close any program that is using ffmpeg and try again.";
pub const E_BUSY: &str = "Another install is already running.";
pub const E_NO_CURL: &str = "curl.exe or tar.exe is missing. They are part of Windows 10 version 1803 and newer.";
pub const E_NO_ASSET: &str = "The download was not found on GitHub.";
pub const E_BLUR: &str = "Blur's installer did not finish. Install it yourself from github.com/f0e/blur.";
pub const E_UNKNOWN: &str = "Nothing to install for that.";

// Notes beside an item.
const N_OLD: &str = "Older than ffmpeg 8: it has no Windows Graphics Capture, so Glint captures with Desktop Duplication. That works, but Graphics Capture is smoother at high frame rates. Reinstall to get the newest ffmpeg.";
const N_NO_RUN: &str = "The file is there but does not run.";
const N_FFMPEG: &str = "Records the replay buffer and saves clips. Required.";
const N_FFPROBE: &str = "Reads clip lengths and sizes for the library. Installed together with ffmpeg.";
const N_BLUR: &str = "Optional: motion-blur renders of finished clips. Windows may ask for permission while it installs.";
const N_WINDOWS: &str = "Part of Windows 10 version 1803 and newer. Only needed to install the parts above.";

/// One program Glint uses.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub required: bool,
    /// "ok" | "missing" | "old"
    pub state: String,
    pub path: String,
    pub version: String,
    pub note: String,
    /// It has an Install button.
    pub installable: bool,
}

/// Sent as the "deps-progress" event while installing.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Progress {
    pub id: String,
    /// "download" | "unpack" | "install" | "done" | "error"
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub error: String,
}

static BUSY: Mutex<Option<String>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// `%LOCALAPPDATA%\Glint\bin`: where ffmpeg goes when Glint's own folder is
/// read-only. `GLINT_BIN_DIR` overrides it (the self-test uses that).
pub fn local_bin_dir() -> PathBuf {
    match std::env::var_os("GLINT_BIN_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Glint").join("bin"),
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
}

/// True if a file can be created in `dir`.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".glint-write-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// Where a new ffmpeg goes: beside Glint.exe when that folder is writable, else `local_bin_dir()`.
pub fn ffmpeg_target() -> PathBuf {
    match exe_dir() {
        Some(d) if writable(&d) => d,
        _ => local_bin_dir(),
    }
}

/// A Windows tool: System32 first, then PATH.
fn system_tool(name: &str) -> Option<PathBuf> {
    let sys = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join(format!("{name}.exe"));
    if sys.is_file() { Some(sys) } else { util::find_exe("", name) }
}

/// Run `exe args`, wait at most `secs`, and return its stdout (None if it did not start or failed).
fn run_out(exe: &Path, args: &[&str], secs: u64) -> Option<String> {
    let mut child = util::hidden_cmd(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < Duration::from_secs(secs) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let buf = reader.join().unwrap_or_default();
    status.success().then(|| String::from_utf8_lossy(&buf).into_owned())
}

/// "8.0.1-essentials_build-www.gyan.dev" out of "ffmpeg version 8.0.1-essentials_build-www.gyan.dev Copyright ...".
fn version_word(text: &str, after: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut words = line.split_whitespace();
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case(after) {
            return words.next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// ffmpeg's state: does it run, and does it have the ` gfxcapture ` filter (ffmpeg 8+).
pub fn ffmpeg_item(path: Option<PathBuf>) -> Item {
    let mut it = Item { id: "ffmpeg".into(), name: "ffmpeg".into(), required: true, installable: true, note: N_FFMPEG.into(), ..Default::default() };
    let Some(p) = path else {
        it.state = "missing".into();
        return it;
    };
    it.path = p.to_string_lossy().into_owned();
    let Some(ver) = run_out(&p, &["-hide_banner", "-version"], 10) else {
        it.state = "missing".into();
        it.note = N_NO_RUN.into();
        return it;
    };
    it.version = version_word(&ver, "version");
    let wgc = run_out(&p, &["-hide_banner", "-filters"], 10).is_some_and(|f| f.contains(" gfxcapture "));
    if wgc {
        it.state = "ok".into();
    } else {
        it.state = "old".into();
        it.note = N_OLD.into();
    }
    it
}

pub fn ffprobe_item(path: Option<PathBuf>) -> Item {
    let mut it = Item { id: "ffprobe".into(), name: "ffprobe".into(), required: true, installable: true, note: N_FFPROBE.into(), ..Default::default() };
    let Some(p) = path else {
        it.state = "missing".into();
        return it;
    };
    it.path = p.to_string_lossy().into_owned();
    match run_out(&p, &["-hide_banner", "-version"], 10) {
        Some(v) => {
            it.state = "ok".into();
            it.version = version_word(&v, "version");
        }
        None => {
            it.state = "missing".into();
            it.note = N_NO_RUN.into();
        }
    }
    it
}

pub fn blur_item(path: Option<PathBuf>) -> Item {
    let mut it = Item { id: "blur".into(), name: "Blur".into(), required: false, installable: true, note: N_BLUR.into(), ..Default::default() };
    match path {
        Some(p) => {
            it.state = "ok".into();
            it.path = p.to_string_lossy().into_owned();
        }
        None => it.state = "missing".into(),
    }
    it
}

fn tool_item(id: &str, path: Option<PathBuf>, version_after: &str) -> Item {
    let mut it = Item { id: id.into(), name: format!("{id}.exe"), required: false, installable: false, note: N_WINDOWS.into(), ..Default::default() };
    match path.as_ref().and_then(|p| run_out(p, &["--version"], 10).map(|v| (p, v))) {
        Some((p, v)) => {
            it.state = "ok".into();
            it.path = p.to_string_lossy().into_owned();
            it.version = version_word(&v, version_after);
        }
        None => {
            it.state = "missing".into();
            it.path = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }
    it
}

/// Every item, with `find(name)` locating ffmpeg and ffprobe (so a test can point it at a folder).
pub fn check_with(find: &dyn Fn(&str) -> Option<PathBuf>, blur: Option<PathBuf>) -> Vec<Item> {
    vec![
        ffmpeg_item(find("ffmpeg")),
        ffprobe_item(find("ffprobe")),
        blur_item(blur),
        tool_item("curl", system_tool("curl"), "curl"),
        // "bsdtar 3.7.2 - libarchive ..."
        tool_item("tar", system_tool("tar"), "bsdtar"),
    ]
}

/// The programs as Glint finds them right now.
pub fn check(s: &Settings) -> Vec<Item> {
    let custom = s.ffmpeg_path.clone();
    check_with(&|name| util::find_exe(if name == "ffmpeg" { custom.as_str() } else { "" }, name), blur::find(s))
}

/* ---------- downloading ---------- */

fn curl() -> std::process::Command {
    let exe = system_tool("curl").unwrap_or_else(|| PathBuf::from("curl.exe"));
    let mut c = util::hidden_cmd(&exe);
    // A generic client name: GitHub requires one, and nothing here should identify the user.
    c.args(["-sS", "-L", "--fail", "--retry", "2", "--connect-timeout", "20", "-H", "User-Agent: Glint"]);
    c.stdin(Stdio::null());
    c
}

/// GET a GitHub API path and parse the JSON.
fn api(path: &str) -> Result<serde_json::Value, String> {
    let out = curl()
        .args(["--max-time", "30", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{path}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            util::log(&format!("deps: curl.exe did not start: {e}"));
            E_NO_CURL.to_string()
        })?;
    if !out.status.success() {
        util::log(&format!("deps: GitHub {path}: {}", util::last_lines(&out.stderr, 1)));
        return Err(E_OFFLINE.into());
    }
    serde_json::from_slice(&out.stdout).map_err(|_| E_OFFLINE.to_string())
}

/// (name, address, size) of the first asset of a release whose name passes `want`.
fn asset(release: &serde_json::Value, want: impl Fn(&str) -> bool) -> Option<(String, String, u64)> {
    release["assets"].as_array()?.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        let url = a["browser_download_url"].as_str()?;
        want(name).then(|| (name.to_string(), url.to_string(), a["size"].as_u64().unwrap_or(0)))
    })
}

/// The newest ffmpeg essentials build: (name, address, size).
pub fn ffmpeg_asset() -> Result<(String, String, u64), String> {
    let release = api(&format!("{FFMPEG_REPO}/releases/latest"))?;
    asset(&release, |n| n.ends_with("essentials_build.zip")).ok_or_else(|| E_NO_ASSET.to_string())
}

/// Blur's installer: (name, address, size).
pub fn blur_asset() -> Result<(String, String, u64), String> {
    let release = api(&format!("{BLUR_REPO}/releases/tags/{BLUR_TAG}"))?;
    asset(&release, |n| n == BLUR_ASSET).ok_or_else(|| E_NO_ASSET.to_string())
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Download `url` to `to`, reporting (bytes so far, total). Stops on `cancel`, after
/// 15 minutes, or when less than 1 KB/s arrives for a whole minute.
pub fn download(url: &str, to: &Path, total: u64, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(E_DOWNLOAD.into());
    }
    let _ = fs::remove_file(to);
    let mut child = curl()
        .args(["--max-time", "900", "--speed-limit", "1024", "--speed-time", "60", "-o"])
        .arg(to)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            util::log(&format!("deps: curl.exe did not start: {e}"));
            E_NO_CURL.to_string()
        })?;
    let mut err = child.stderr.take();
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(e) = err.as_mut() {
            let _ = e.read_to_end(&mut buf);
        }
        buf
    });
    progress(0, total);
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            kill(&mut child);
            let _ = fs::remove_file(to);
            return Err(E_CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(200));
                progress(fs::metadata(to).map(|m| m.len()).unwrap_or(0), total);
            }
            Err(_) => {
                kill(&mut child);
                return Err(E_DOWNLOAD.into());
            }
        }
    };
    let stderr = err_reader.join().unwrap_or_default();
    let got = fs::metadata(to).map(|m| m.len()).unwrap_or(0);
    if !status.success() || got == 0 || (total > 0 && got != total) {
        util::log(&format!("deps: download {url} failed ({got} of {total} bytes): {}", util::last_lines(&stderr, 1)));
        let _ = fs::remove_file(to);
        return Err(E_DOWNLOAD.into());
    }
    progress(got, total.max(got));
    Ok(())
}

/// Unpack a zip with tar.exe. `only` limits it to matching paths.
fn unzip(zip: &Path, into: &Path, only: &[&str]) -> bool {
    let _ = fs::create_dir_all(into);
    let Some(tar) = system_tool("tar") else { return false };
    util::hidden_cmd(&tar).arg("-xf").arg(zip).arg("-C").arg(into).args(only).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn find_file(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_file() && p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)) {
            return Some(p);
        }
        if p.is_dir() && depth > 0 {
            if let Some(found) = find_file(&p, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Copy `src` over `dst`, as setup.rs does: a file in use cannot be overwritten
/// on Windows but can be renamed, so an existing one is moved aside to `.old` first.
pub fn put(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        let old = PathBuf::from(format!("{}.old", dst.display()));
        let _ = fs::remove_file(&old);
        fs::rename(dst, &old).map_err(|e| format!("Could not replace {}: {e}", dst.display()))?;
        let _ = fs::remove_file(&old); // fails harmlessly while the old file is still running
    }
    fs::copy(src, dst).map(|_| ()).map_err(|e| format!("Could not write {}: {e}", dst.display()))
}

/// Download the newest ffmpeg essentials build and put ffmpeg.exe and ffprobe.exe into `dir`.
/// Both are checked to run before either is put in place.
pub fn install_ffmpeg_into(dir: &Path, work: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(&str, u64, u64)) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|_| E_PUT.to_string())?;
    let (name, url, size) = ffmpeg_asset()?;
    util::log(&format!("deps: downloading {name} ({size} bytes)"));
    let zip = work.join(&name);
    download(&url, &zip, size, cancel, &mut |d, t| progress("download", d, t))?;
    if cancel.load(Ordering::Relaxed) {
        return Err(E_CANCELLED.into());
    }
    progress("unpack", size, size);
    let unpacked = work.join("unpacked");
    // Only the two programs are needed out of the archive.
    if !unzip(&zip, &unpacked, &["*/bin/ffmpeg.exe", "*/bin/ffprobe.exe"]) {
        unzip(&zip, &unpacked, &[]);
    }
    let _ = fs::remove_file(&zip);
    let mut found = Vec::new();
    for exe in ["ffmpeg.exe", "ffprobe.exe"] {
        let p = find_file(&unpacked, exe, 4).ok_or_else(|| E_UNPACK.to_string())?;
        if run_out(&p, &["-hide_banner", "-version"], 20).is_none() {
            return Err(E_NO_RUN.into());
        }
        found.push((p, dir.join(exe)));
    }
    progress("install", size, size);
    fs::create_dir_all(dir).map_err(|_| E_PUT.to_string())?;
    for (src, dst) in &found {
        put(src, dst).map_err(|e| {
            util::log(&format!("deps: {e}"));
            E_PUT.to_string()
        })?;
    }
    util::log(&format!("deps: ffmpeg and ffprobe installed into {}", dir.display()));
    Ok(())
}

/// Download Blur's installer and run it unattended into `%LOCALAPPDATA%\Glint\blur`.
/// Windows may show a permission prompt: only ever run this from a click.
fn install_blur(work: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(&str, u64, u64)) -> Result<(), String> {
    fs::create_dir_all(work).map_err(|_| E_PUT.to_string())?;
    let (name, url, size) = blur_asset()?;
    let exe = work.join(&name);
    download(&url, &exe, size, cancel, &mut |d, t| progress("download", d, t))?;
    progress("install", size, size);
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("Glint").join("blur");
    // Inno Setup's standard unattended switches, as in setup.rs.
    let mut child = util::hidden_cmd(&exe)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"])
        .arg(format!("/DIR={}", dir.display()))
        .spawn()
        .map_err(|e| {
            util::log(&format!("deps: Blur's installer did not start: {e}"));
            E_BLUR.to_string()
        })?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(900) => std::thread::sleep(Duration::from_millis(300)),
            _ => {
                kill(&mut child);
                return Err(E_BLUR.into());
            }
        }
    }
    if dir.join("blur-cli.exe").is_file() { Ok(()) } else { Err(E_BLUR.into()) }
}

/// Install `id` ("ffmpeg" or "blur"), reporting through `progress(phase, done, total)`.
/// One install at a time; `cancel()` stops the download.
pub fn install(id: &str, progress: &mut dyn FnMut(&str, u64, u64)) -> Result<(), String> {
    {
        let mut busy = BUSY.lock().unwrap();
        if busy.is_some() {
            return Err(E_BUSY.into());
        }
        *busy = Some(id.to_string());
    }
    CANCEL.store(false, Ordering::SeqCst);
    let work = util::tmp_dir().join(format!("deps-{}", util::new_id()));
    let result = match id {
        "ffmpeg" | "ffprobe" => install_ffmpeg_into(&ffmpeg_target(), &work, &CANCEL, progress),
        "blur" => install_blur(&work, &CANCEL, progress),
        _ => Err(E_UNKNOWN.into()),
    };
    let _ = fs::remove_dir_all(&work);
    *BUSY.lock().unwrap() = None;
    if let Err(e) = &result {
        util::log(&format!("deps: install {id}: {e}"));
    }
    result
}

/// Stop the running install's download. False if nothing is installing.
pub fn cancel() -> bool {
    CANCEL.store(true, Ordering::SeqCst);
    BUSY.lock().unwrap().is_some()
}

/// What is installing now ("" = nothing).
pub fn installing() -> String {
    BUSY.lock().unwrap().clone().unwrap_or_default()
}

/// After ffmpeg was installed: start using it without a restart of Glint.
pub fn use_new_ffmpeg(core: &Core) {
    let mut s = core.settings();
    // A path set by hand to some other ffmpeg would still win over the new one.
    if !s.ffmpeg_path.trim().is_empty() {
        let mine = ffmpeg_target().join("ffmpeg.exe");
        if util::find_exe(&s.ffmpeg_path, "ffmpeg").is_some_and(|p| p != mine) {
            s.ffmpeg_path.clear();
            core.apply_settings(s); // restarts the captures whose ffmpeg path changed
            core.emit("settings-changed", ());
        }
    }
    if core.rec.is_running() {
        let cfgs = crate::core::capture_cfgs(&core.settings());
        if core.rec.current_cfgs() != cfgs {
            core.rec.start(cfgs); // the path changed: only those captures restart
        } else {
            // Same path, new file: the old ffmpeg keeps running from its `.old` copy. Restart only
            // when that capture is failing or lacks Graphics Capture (an old ffmpeg replaced), so a
            // plain reinstall does not empty a working buffer.
            let st = core.rec.stats();
            if !st.status.error.is_empty() || st.status.method != "wgc" {
                core.rec.stop();
                core.start_recording();
            }
        }
    }
    core.emit("status-changed", ());
}

/// Run an install for the UI: progress as "deps-progress" events, then the capture picks the new ffmpeg up.
pub fn install_for_ui(core: &Core, id: &str) -> Result<Vec<Item>, String> {
    let emit = |p: Progress| core.emit("deps-progress", p);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut on = |phase: &str, done: u64, total: u64| {
        // At most about eight events a second while downloading.
        if phase != "download" || last.elapsed() >= Duration::from_millis(120) || (total > 0 && done >= total) {
            last = Instant::now();
            emit(Progress { id: id.into(), phase: phase.into(), done, total, error: String::new() });
        }
    };
    let result = install(id, &mut on);
    match &result {
        Ok(()) => {
            if id == "ffmpeg" || id == "ffprobe" {
                use_new_ffmpeg(core);
            }
            emit(Progress { id: id.into(), phase: "done".into(), ..Default::default() });
        }
        Err(e) => emit(Progress { id: id.into(), phase: "error".into(), error: e.clone(), ..Default::default() }),
    }
    result.map(|_| check(&core.settings()))
}

/* ---------- self-test ---------- */

fn err_is(r: &Result<(), String>, e: &str) -> bool {
    r.as_ref().err().map(String::as_str) == Some(e)
}

/// A stand-in for an old ffmpeg, for the self-test: when this program runs as
/// `ffmpeg.exe` with `GLINT_FAKE_FFMPEG=1`, it answers like an ffmpeg 6 (no
/// gfxcapture) and exits. Returns true if it did.
pub fn fake_ffmpeg() -> bool {
    let me = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()));
    if me.as_deref() != Some("ffmpeg") || std::env::var_os("GLINT_FAKE_FFMPEG").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return false;
    }
    use std::io::Write;
    let args: Vec<String> = std::env::args().collect();
    let mut out = std::io::stdout();
    if args.iter().any(|a| a == "-version") {
        let _ = writeln!(out, "ffmpeg version 6.1-fake Copyright (c) 2000-2023 the FFmpeg developers");
    } else if args.iter().any(|a| a == "-filters") {
        let _ = writeln!(out, "Filters:\n ... ddagrab           |->V       Grab Windows Desktop images using Desktop Duplication API\n ... scale             V->V       Scale the input video size");
    }
    let _ = out.flush();
    if args.iter().any(|a| a == "--deps-test-hold") {
        // Stay running (a file in use) until the test kills it; gone by itself after two minutes.
        std::thread::sleep(Duration::from_secs(120));
    }
    true
}

/// `glint.exe --deps-test <report.txt>`: headless checks. Downloads the ffmpeg
/// essentials build once (about 100 MB) into a scratch folder under
/// `GLINT_DATA_DIR`; never touches `%LOCALAPPDATA%\Glint`, never runs Blur's installer.
pub fn self_test(report_path: &str) {
    let mut lines: Vec<String> = Vec::new();
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let check = |lines: &mut Vec<String>, name: &str, ok: bool| lines.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));
    let root = util::tmp_dir().join(format!("deps-test-{}", util::new_id()));
    let _ = fs::create_dir_all(&root);
    let in_dir = |dir: PathBuf| move |name: &str| Some(dir.join(format!("{name}.exe"))).filter(|p| p.is_file());

    // 1. An empty folder: nothing found.
    let empty = root.join("empty");
    let _ = fs::create_dir_all(&empty);
    let items = check_with(&in_dir(empty.clone()), None);
    let state = |items: &[Item], id: &str| items.iter().find(|i| i.id == id).map(|i| i.state.clone()).unwrap_or_default();
    check(&mut lines, "empty folder: ffmpeg missing", state(&items, "ffmpeg") == "missing");
    check(&mut lines, "empty folder: ffprobe missing", state(&items, "ffprobe") == "missing");
    check(&mut lines, "empty folder: Blur missing and optional", state(&items, "blur") == "missing" && items.iter().any(|i| i.id == "blur" && !i.required && i.installable));
    check(&mut lines, "curl.exe found and runs", state(&items, "curl") == "ok");
    check(&mut lines, "tar.exe found and runs", state(&items, "tar") == "ok");
    for i in &items {
        lines.push(format!("INFO {} state={} version={:?} path={:?}", i.id, i.state, i.version, i.path));
    }
    check(&mut lines, "five items, ffmpeg and ffprobe required", items.len() == 5 && items.iter().filter(|i| i.required).count() == 2);

    // 2. A file that does not run.
    let broken = root.join("broken");
    let _ = fs::create_dir_all(&broken);
    let _ = fs::write(broken.join("ffmpeg.exe"), b"not a program");
    let it = ffmpeg_item(in_dir(broken.clone())("ffmpeg"));
    check(&mut lines, "a file that does not run counts as missing", it.state == "missing" && it.note == N_NO_RUN && !it.path.is_empty());

    // 3. An old ffmpeg (this program, answering as ffmpeg 6 without gfxcapture).
    // The DLLs beside this program go too: a copy that cannot load WebView2Loader.dll would make
    // Windows show an error box.
    let fake_into = |dir: &Path| -> bool {
        let _ = fs::create_dir_all(dir);
        let Ok(me) = std::env::current_exe() else { return false };
        for e in me.parent().and_then(|d| fs::read_dir(d).ok()).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                let _ = fs::copy(e.path(), dir.join(e.file_name()));
            }
        }
        fs::copy(&me, dir.join("ffmpeg.exe")).is_ok()
    };
    let old = root.join("old");
    let _ = fake_into(&old);
    std::env::set_var("GLINT_FAKE_FFMPEG", "1");
    let it = ffmpeg_item(in_dir(old.clone())("ffmpeg"));
    std::env::remove_var("GLINT_FAKE_FFMPEG");
    lines.push(format!("INFO old ffmpeg: state={} version={:?}", it.state, it.version));
    check(&mut lines, "an ffmpeg without gfxcapture is \"old\", with the fallback in its note", it.state == "old" && it.version == "6.1-fake" && it.note.contains("Desktop Duplication"));

    // 4. The real ffmpeg on this PC.
    let real = util::find_exe("", "ffmpeg");
    let it = ffmpeg_item(real.clone());
    lines.push(format!("INFO this PC's ffmpeg: {:?} state={} version={:?}", real, it.state, it.version));
    check(&mut lines, "this PC's ffmpeg is ok (8+, gfxcapture)", it.state == "ok" && !it.version.is_empty());

    // 5. find_exe looks in the per-user bin folder (GLINT_BIN_DIR stands in for %LOCALAPPDATA%\Glint\bin).
    let default_bin = {
        let saved = std::env::var_os("GLINT_BIN_DIR");
        std::env::remove_var("GLINT_BIN_DIR");
        let d = local_bin_dir();
        if let Some(v) = saved {
            std::env::set_var("GLINT_BIN_DIR", v);
        }
        d
    };
    check(&mut lines, "the per-user folder is %LOCALAPPDATA%\\Glint\\bin", default_bin.ends_with("Glint\\bin") && default_bin.starts_with(std::env::var_os("LOCALAPPDATA").unwrap_or_default()));
    let bin = root.join("bin");
    let _ = fs::create_dir_all(&bin);
    let probe = bin.join("glintdepsprobe.exe");
    let _ = fs::write(&probe, b"x");
    std::env::set_var("GLINT_BIN_DIR", &bin);
    check(&mut lines, "find_exe finds a program in the per-user folder", util::find_exe("", "glintdepsprobe").as_deref() == Some(probe.as_path()));
    check(&mut lines, "local_bin_dir follows GLINT_BIN_DIR", local_bin_dir() == bin);
    std::env::remove_var("GLINT_BIN_DIR");
    check(&mut lines, "find_exe without it does not find that program", util::find_exe("", "glintdepsprobe").is_none());
    check(&mut lines, "a folder can be told writable", writable(&root) && !writable(&probe.join("sub")));

    // 6. Errors: a download that cannot connect, cancelling, and only one install at a time.
    let never = AtomicBool::new(false);
    let r = download("https://127.0.0.1:9/nothing.zip", &root.join("x.zip"), 0, &never, &mut |_, _| {});
    check(&mut lines, "a download that cannot connect fails with a plain message", err_is(&r, E_DOWNLOAD) && !root.join("x.zip").exists());
    let r = download("http://example.com/x.zip", &root.join("x.zip"), 0, &never, &mut |_, _| {});
    check(&mut lines, "only https addresses are fetched", r.is_err());
    *BUSY.lock().unwrap() = Some("ffmpeg".into());
    let r = install("blur", &mut |_, _, _| {});
    check(&mut lines, "a second install is refused while one runs", err_is(&r, E_BUSY));
    check(&mut lines, "cancel reports a running install", cancel() && installing() == "ffmpeg");
    *BUSY.lock().unwrap() = None;
    check(&mut lines, "an unknown item is refused", err_is(&install("nothing", &mut |_, _, _| {}), E_UNKNOWN) && installing().is_empty());

    // 7. Blur: the release and its installer resolve (never downloaded or run here).
    match blur_asset() {
        Ok((name, url, size)) => {
            lines.push(format!("INFO Blur asset {name} {size} bytes"));
            check(&mut lines, "Blur v2.23's installer is on GitHub", name == BLUR_ASSET && url.starts_with("https://github.com/") && size > 1_000_000);
        }
        Err(e) => check(&mut lines, &format!("Blur v2.23's installer is on GitHub ({e})"), false),
    }

    // 8. The real thing: cancel one download early, then install ffmpeg into a scratch folder.
    match ffmpeg_asset() {
        Err(e) => check(&mut lines, &format!("the ffmpeg build is on GitHub ({e})"), false),
        Ok((name, url, size)) => {
            lines.push(format!("INFO ffmpeg asset {name} {size} bytes"));
            check(&mut lines, "the ffmpeg build is on GitHub", url.starts_with("https://github.com/") && size > 30_000_000);
            let stop = AtomicBool::new(false);
            let part = root.join("cancel.zip");
            let started = Instant::now();
            let r = download(&url, &part, size, &stop, &mut |d, _| {
                if d > 0 || started.elapsed() > Duration::from_secs(20) {
                    stop.store(true, Ordering::Relaxed);
                }
            });
            check(&mut lines, &format!("cancelling a download stops it ({:.1} s) and removes the file", started.elapsed().as_secs_f64()), err_is(&r, E_CANCELLED) && !part.exists());
        }
    }
    let dest = root.join("install");
    // An "old" ffmpeg already there and running, as an in-use file to replace.
    let _ = fake_into(&dest);
    let mut running = util::hidden_cmd(&dest.join("ffmpeg.exe")).arg("--deps-test-hold").env("GLINT_FAKE_FFMPEG", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
    std::thread::sleep(Duration::from_millis(500));
    let in_use = running.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)));
    check(&mut lines, "the old ffmpeg in the target folder is running (a file in use)", in_use);
    let mut steps: Vec<(String, u64, u64)> = Vec::new();
    let started = Instant::now();
    let r = install_ffmpeg_into(&dest, &root.join("work"), &AtomicBool::new(false), &mut |ph, d, t| steps.push((ph.to_string(), d, t)));
    let secs = started.elapsed().as_secs_f64();
    if let Some(mut c) = running {
        kill(&mut c);
    }
    lines.push(format!("INFO install took {secs:.1} s, {} progress reports", steps.len()));
    check(&mut lines, &format!("ffmpeg installs into a scratch folder ({r:?})"), r.is_ok());
    let downloads: Vec<&(String, u64, u64)> = steps.iter().filter(|s| s.0 == "download").collect();
    let rising = downloads.windows(2).all(|w| w[1].1 >= w[0].1);
    let full = downloads.last().is_some_and(|s| s.1 == s.2 && s.2 > 0);
    check(&mut lines, "progress rises to the asset's full size", rising && full && downloads.len() > 2);
    check(&mut lines, "then unpack and install steps", steps.iter().any(|s| s.0 == "unpack") && steps.last().is_some_and(|s| s.0 == "install"));
    let it = ffmpeg_item(Some(dest.join("ffmpeg.exe")));
    lines.push(format!("INFO installed ffmpeg: state={} version={:?}", it.state, it.version));
    check(&mut lines, "the installed ffmpeg runs and has gfxcapture", it.state == "ok");
    check(&mut lines, "the installed ffprobe runs", ffprobe_item(Some(dest.join("ffprobe.exe"))).state == "ok");
    check(&mut lines, "the scratch download folder is left clean of the zip", fs::read_dir(root.join("work")).map(|d| d.flatten().all(|e| e.path().is_dir())).unwrap_or(true));

    let _ = fs::remove_dir_all(&root);
    let pass = lines.iter().filter(|l| l.starts_with("PASS")).count();
    let fail = lines.iter().filter(|l| l.starts_with("FAIL")).count();
    lines.push(format!("{pass} pass, {fail} fail"));
    let _ = fs::write(report_path, lines.join("\n") + "\n");
}
