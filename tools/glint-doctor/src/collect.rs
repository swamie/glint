//! What Glint Doctor reads on this PC. Read only: nothing here changes a file
//! or a setting of Glint.
//!
//! Where things are is taken from Glint's own code:
//!   - the data folder: `%APPDATA%\Glint`, or `GLINT_DATA_DIR` (util.rs `data_dir`)
//!     with `glint.log`, `settings.json`, `update.json`, `library.json`, ...
//!   - the install folder and version: the uninstall entry GlintSetup.exe writes
//!     (setup.rs `UNINSTALL_KEY`), else `%LOCALAPPDATA%\Programs\Glint`
//!   - ffmpeg: the path in the settings, beside Glint.exe,
//!     `%LOCALAPPDATA%\Glint\bin`, then PATH (util.rs `find_exe`)
//!   - Windows version, graphics cards and CPU: the same registry values as
//!     bugreport.rs and win.rs `hardware`
//! Plus what Glint does not record itself: crashes of Glint.exe from the
//! Windows event log, and whether other clipping software is running.
//!
//! Everything collected goes through `Scrubber` (scrub.rs) before it is shown
//! or sent.
use crate::scrub::Scrubber;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The upload may not be larger than this (the server refuses more than 400 KB).
pub const MAX_BUNDLE: usize = 360 * 1024;
/// The end of glint.log: at most this many lines and bytes.
const LOG_LINES: usize = 700;
const LOG_BYTES: usize = 140 * 1024;
/// Lines of the whole log that look like a fault, newest last.
const ERROR_LINES: usize = 120;
const ERROR_BYTES: usize = 40 * 1024;
const ERROR_WORDS: [&str; 22] = [
    "error", "fail", "could not", "cannot", "can't", "not found", "missing", "denied", "refused", "panic", "crash", "timed out", "timeout", "invalid", "unsupported", "lost", "gave up", "exit code", "exited", "0x8", "0xc0", "retry",
];

/// Other programs that record the screen or hook games; they can get in Glint's way.
const OTHER_RECORDERS: [&str; 22] = [
    "obs64.exe", "obs32.exe", "streamlabs obs.exe", "medal.exe", "medalencoder.exe", "outplayed.exe", "overwolf.exe", "nvidia share.exe", "nvidia overlay.exe", "nvcontainer.exe", "amdrsserv.exe", "radeonsoftware.exe", "gamebar.exe",
    "gamebarftserver.exe", "xboxgamebar.exe", "steelseriesgg.exe", "steelseriesengine.exe", "action.exe", "bandicam.exe", "bdcam.exe", "discord.exe", "rivatunerstatisticsserver.exe",
];

/// Glint's data folder, as util.rs `data_dir` finds it (but never created here).
pub fn data_dir() -> PathBuf {
    match std::env::var_os("GLINT_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Glint"),
    }
}

fn env_dir(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn now_secs() -> u64 {
    unix_secs(SystemTime::now())
}

/// Run a program and return what it printed, or None if it did not run or took too long.
fn run(program: &Path, args: &[&str], secs: u64) -> Option<String> {
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        buf
    });
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < end => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let bytes = reader.join().ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

// ---- the registry (Windows only; elsewhere these find nothing, so the rest can be built and tested anywhere)

#[cfg(windows)]
mod reg {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    pub fn hklm(path: &str, name: &str) -> String {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(path).and_then(|k| k.get_value::<String, _>(name)).unwrap_or_default()
    }
    pub fn hkcu(path: &str, name: &str) -> String {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(path).and_then(|k| k.get_value::<String, _>(name)).unwrap_or_default()
    }
    pub fn hklm_u32(path: &str, name: &str) -> Option<u32> {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(path).and_then(|k| k.get_value::<u32, _>(name)).ok()
    }
    /// The numbered subkeys ("0000", "0001", ...) of a device class.
    pub fn hklm_subkeys(path: &str) -> Vec<String> {
        RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(path).map(|k| k.enum_keys().flatten().collect()).unwrap_or_default()
    }
}

#[cfg(not(windows))]
mod reg {
    pub fn hklm(_: &str, _: &str) -> String {
        String::new()
    }
    pub fn hkcu(_: &str, _: &str) -> String {
        String::new()
    }
    pub fn hklm_u32(_: &str, _: &str) -> Option<u32> {
        None
    }
    pub fn hklm_subkeys(_: &str) -> Vec<String> {
        Vec::new()
    }
}

// ---- memory and disk space (two kernel32 calls, declared here to keep the program small)

#[cfg(windows)]
mod sys {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        fn GetDiskFreeSpaceExW(dir: *const u16, free_for_caller: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }

    /// (installed, free) memory in MB.
    pub fn memory_mb() -> Option<(u64, u64)> {
        let mut m: MemoryStatusEx = unsafe { std::mem::zeroed() };
        m.length = std::mem::size_of::<MemoryStatusEx>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then(|| (m.total_phys >> 20, m.avail_phys >> 20))
    }

    /// (free, total) space of the drive `dir` is on, in MB.
    pub fn disk_mb(dir: &Path) -> Option<(u64, u64)> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let (mut free, mut total) = (0u64, 0u64);
        (unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } != 0).then(|| (free >> 20, total >> 20))
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn memory_mb() -> Option<(u64, u64)> {
        None
    }
    pub fn disk_mb(_: &std::path::Path) -> Option<(u64, u64)> {
        None
    }
}

// ---- the parts of the report

/// "Windows 11 Pro 24H2 (build 26100.2314)", the same way bugreport.rs words it.
fn windows_version() -> String {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = reg::hklm(KEY, "CurrentBuildNumber");
    if build.is_empty() {
        return "Windows (version unknown)".into();
    }
    let mut product = reg::hklm(KEY, "ProductName");
    // Windows 11 still calls itself "Windows 10" here; the build number tells them apart.
    if build.parse::<u32>().unwrap_or(0) >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    let ubr = reg::hklm_u32(KEY, "UBR").map(|u| format!(".{u}")).unwrap_or_default();
    let display = reg::hklm(KEY, "DisplayVersion");
    let display = if display.is_empty() { String::new() } else { format!(" {display}") };
    format!("{}{display} (build {build}{ubr})", if product.is_empty() { "Windows".to_string() } else { product })
}

/// Installed display adapters with their driver version and date, and the CPU (win.rs `hardware`).
fn hardware() -> Value {
    const CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let mut gpus: Vec<Value> = Vec::new();
    for sub in reg::hklm_subkeys(CLASS).into_iter().filter(|k| k.len() == 4 && k.chars().all(|c| c.is_ascii_digit())) {
        let path = format!(r"{CLASS}\{sub}");
        let name = reg::hklm(&path, "DriverDesc");
        if name.is_empty() || gpus.iter().any(|g| g["name"] == name.as_str()) {
            continue;
        }
        let id = reg::hklm(&path, "MatchingDeviceId").to_lowercase();
        let vendor = if id.contains("ven_10de") {
            "nvidia"
        } else if id.contains("ven_1002") || id.contains("ven_1022") {
            "amd"
        } else if id.contains("ven_8086") {
            "intel"
        } else {
            "other"
        };
        gpus.push(json!({ "name": name, "vendor": vendor, "driver_version": reg::hklm(&path, "DriverVersion"), "driver_date": reg::hklm(&path, "DriverDate") }));
    }
    let cpu = reg::hklm(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").split_whitespace().collect::<Vec<_>>().join(" ");
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    let memory = sys::memory_mb().map(|(total, free)| json!({ "installed_mb": total, "free_mb": free })).unwrap_or(Value::Null);
    json!({ "gpus": gpus, "cpu": cpu, "logical_cores": cores, "memory": memory })
}

/// The Edge WebView2 runtime Glint's window runs in ("" = not found).
fn webview2_version() -> String {
    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    [reg::hklm(&format!(r"SOFTWARE\WOW6432Node\{CLIENT}"), "pv"), reg::hklm(&format!(r"SOFTWARE\{CLIENT}"), "pv"), reg::hkcu(&format!(r"Software\{CLIENT}"), "pv")].into_iter().find(|v| !v.is_empty() && v != "0.0.0.0").unwrap_or_default()
}

/// Where Glint is installed and which version, from the uninstall entry and update.json.
struct Install {
    dir: Option<PathBuf>,
    version: String,
    info: Value,
}

fn install(data: &Path) -> Install {
    const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glint";
    let registered_dir = reg::hkcu(UNINSTALL, "InstallLocation");
    let registered_version = reg::hkcu(UNINSTALL, "DisplayVersion");
    let update: Value = fs::read_to_string(data.join("update.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let text = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let mut places: Vec<PathBuf> = Vec::new();
    if !registered_dir.is_empty() {
        places.push(PathBuf::from(&registered_dir));
    }
    places.push(env_dir("LOCALAPPDATA").join("Programs").join("Glint"));
    if let Some(here) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        places.push(here);
    }
    let dir = places.into_iter().find(|d| d.join("Glint.exe").is_file());
    // update.json holds the version of the Glint that last ran; the uninstall entry the one that was installed.
    let ran = text(&update, "current");
    let version = if !ran.is_empty() { ran.clone() } else { registered_version.clone() };
    let files: Vec<Value> = dir.as_ref().map(|d| list_files(d, 40)).unwrap_or_default();
    let info = json!({
        "version": version,
        "version_last_run": ran,
        "version_installed": registered_version,
        "install_dir": dir.as_ref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default(),
        "install_dir_files": files,
        "update": { "latest": text(&update, "latest"), "available": update["available"].as_bool().unwrap_or(false), "last_check_unix": update["checked"].as_u64().unwrap_or(0), "error": text(&update, "error") },
    });
    Install { dir, version, info }
}

/// The files directly in `dir`: name, size, age in hours. Folders: name and how many entries.
fn list_files(dir: &Path, max: usize) -> Vec<Value> {
    let now = now_secs();
    let mut out: Vec<Value> = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for e in entries.flatten().take(max) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            let n = fs::read_dir(e.path()).map(|d| d.count()).unwrap_or(0);
            out.push(json!({ "folder": name, "entries": n }));
        } else {
            let age = meta.modified().ok().map(|t| now.saturating_sub(unix_secs(t)) / 3600);
            out.push(json!({ "file": name, "bytes": meta.len(), "modified_hours_ago": age }));
        }
    }
    out
}

/// How many entries a JSON file of Glint's holds (a list, or an object with one list in it).
fn count_entries(path: &Path) -> Value {
    let Ok(text) = fs::read_to_string(path) else { return Value::Null };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(a)) => json!(a.len()),
        Ok(Value::Object(o)) => o.values().filter_map(|v| v.as_array().map(|a| json!(a.len()))).next().unwrap_or(json!(o.len())),
        Ok(_) => Value::Null,
        Err(e) => json!(format!("not readable as JSON: {e}")),
    }
}

/// The end of a file as lines: at most `lines` of them and `bytes` bytes, cut at a line start.
fn tail(text: &str, lines: usize, bytes: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let mut from = all.len().saturating_sub(lines);
    let mut size: usize = all[from..].iter().map(|l| l.len() + 1).sum();
    while size > bytes && from < all.len() {
        size -= all[from].len() + 1;
        from += 1;
    }
    all[from..].join("\n")
}

/// The lines of the whole log that look like a fault, each once (with how often it came), newest last.
fn fault_lines(text: &str) -> String {
    // "2026-11-18 12:34:56.789 message": the same message at another time is the same line.
    let message = |l: &str| -> String { l.splitn(3, ' ').nth(2).unwrap_or(l).to_string() };
    let mut seen: Vec<(String, String, usize)> = Vec::new();
    for line in text.lines() {
        let low = line.to_lowercase();
        if !ERROR_WORDS.iter().any(|w| low.contains(w)) {
            continue;
        }
        let msg = message(line);
        if let Some(pos) = seen.iter().position(|(m, _, _)| *m == msg) {
            let (m, _, n) = seen.remove(pos);
            seen.push((m, line.to_string(), n + 1));
        } else {
            seen.push((msg, line.to_string(), 1));
        }
    }
    let lines: Vec<String> = seen.into_iter().map(|(_, line, n)| if n > 1 { format!("{line}   [{n} times, this is the last]") } else { line }).collect();
    tail(&lines.join("\n"), ERROR_LINES, ERROR_BYTES)
}

fn free_space(label: &str, dir: &Path) -> Value {
    // The folder may not exist yet; its nearest existing parent is on the same drive.
    let mut d = dir.to_path_buf();
    while !d.is_dir() {
        if !d.pop() {
            break;
        }
    }
    match sys::disk_mb(&d) {
        Some((free, total)) => json!({ "for": label, "folder": dir.to_string_lossy(), "exists": dir.is_dir(), "free_mb": free, "drive_mb": total }),
        None => json!({ "for": label, "folder": dir.to_string_lossy(), "exists": dir.is_dir() }),
    }
}

/// ffmpeg as Glint would find it: where, which version, which hardware encoders it offers.
fn ffmpeg(settings: &Value, install_dir: Option<&Path>) -> Value {
    let mut places: Vec<(String, PathBuf)> = Vec::new();
    let custom = settings["ffmpeg_path"].as_str().unwrap_or("").trim().to_string();
    if !custom.is_empty() {
        places.push(("settings".into(), PathBuf::from(&custom)));
    }
    let local_bin = match std::env::var_os("GLINT_BIN_DIR") {
        Some(d) => PathBuf::from(d),
        None => env_dir("LOCALAPPDATA").join("Glint").join("bin"),
    };
    places.push(("per-user bin".into(), local_bin.join("ffmpeg.exe")));
    if let Some(d) = install_dir {
        places.push(("beside Glint.exe".into(), d.join("ffmpeg.exe")));
    }
    for d in std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default() {
        places.push(("PATH".into(), d.join("ffmpeg.exe")));
    }
    let found: Vec<(String, PathBuf)> = places.into_iter().filter(|(_, p)| p.is_file()).collect();
    let copies: Vec<Value> = found
        .iter()
        .take(6)
        .map(|(place, path)| {
            let version = run(path, &["-hide_banner", "-version"], 8).and_then(|t| t.lines().next().map(str::to_string)).unwrap_or_else(|| "did not run".into());
            json!({ "where": place, "path": path.to_string_lossy(), "version": version })
        })
        .collect();
    let encoders = found.first().and_then(|(_, p)| run(p, &["-hide_banner", "-encoders"], 8)).map(|t| {
        t.lines().filter_map(|l| l.split_whitespace().nth(1)).filter(|n| ["nvenc", "amf", "qsv", "libx264", "libx265", "libsvtav1", "mf"].iter().any(|k| n.contains(k))).map(str::to_string).collect::<Vec<_>>()
    });
    let ffprobe = found.first().map(|(_, p)| p.with_file_name("ffprobe.exe").is_file());
    json!({ "copies": copies, "encoders_of_first": encoders, "ffprobe_beside_first": ffprobe, "custom_path_set": !custom.is_empty() })
}

/// Is Glint running, and which other recorders are.
fn processes() -> Value {
    let Some(text) = run(Path::new("tasklist.exe"), &["/FO", "CSV", "/NH"], 15) else { return json!({ "error": "tasklist.exe did not run" }) };
    let names: Vec<String> = text.lines().filter_map(|l| l.split("\",\"").next()).map(|n| n.trim_matches('"').to_lowercase()).collect();
    let glint = names.iter().filter(|n| *n == "glint.exe").count();
    let ffmpegs = names.iter().filter(|n| *n == "ffmpeg.exe").count();
    let mut others: Vec<&str> = OTHER_RECORDERS.iter().copied().filter(|r| names.iter().any(|n| n == r)).collect();
    others.dedup();
    json!({ "glint_running": glint, "ffmpeg_running": ffmpegs, "other_recorders_running": others })
}

/// Crashes and hangs of Glint that Windows recorded: the newest few "Application Error" (1000)
/// and "Application Hang" (1002) events that name glint.exe or its ffmpeg.
fn crash_events() -> Value {
    let query = "*[System[(EventID=1000 or EventID=1002) and TimeCreated[timediff(@SystemTime) <= 2592000000]]]";
    let Some(text) = run(Path::new("wevtutil.exe"), &["qe", "Application", &format!("/q:{query}"), "/c:300", "/rd:true", "/f:text"], 20) else {
        return json!({ "error": "the Windows event log could not be read" });
    };
    let mut events: Vec<String> = Vec::new();
    for block in text.split("\nEvent[").filter(|b| !b.trim().is_empty()) {
        let low = block.to_lowercase();
        if !(low.contains("glint") || low.contains("ffmpeg.exe") || low.contains("blur")) {
            continue;
        }
        let kept: Vec<&str> = block.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("User:") && !l.starts_with("User Name:") && !l.starts_with("Computer:") && !l.starts_with("Log Name:") && !l.starts_with("Opcode") && !l.starts_with("Keyword") && !l.starts_with("Task:")).collect();
        events.push(kept.join("\n").chars().take(1500).collect());
        if events.len() == 6 {
            break;
        }
    }
    // Crash dumps Windows kept, if the user turned those on: names and sizes only, never the contents.
    let dumps: Vec<Value> = list_files(&env_dir("LOCALAPPDATA").join("CrashDumps"), 400).into_iter().filter(|f| f["file"].as_str().is_some_and(|n| n.to_lowercase().starts_with("glint"))).take(10).collect();
    json!({ "events_last_30_days": events, "dump_files": dumps })
}

/// Everything, scrubbed. `description` is what the user typed about the problem.
pub fn bundle(scrub: &Scrubber, description: &str) -> Value {
    let data = data_dir();
    let inst = install(&data);
    let settings_raw: Value = fs::read_to_string(data.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    let log_raw = fs::read(data.join("glint.log")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();

    let dir_of = |key: &str| settings_raw[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut disks = vec![free_space("Glint's data", &data)];
    if let Some(d) = dir_of("clips_dir") {
        disks.push(free_space("clips", &d));
    }
    if let Some(d) = dir_of("buffer_dir") {
        disks.push(free_space("replay buffer on disk", &d));
    }

    let mut data_files = Map::new();
    data_files.insert("folder".into(), json!(data.to_string_lossy()));
    data_files.insert("exists".into(), json!(data.is_dir()));
    data_files.insert("contents".into(), json!(list_files(&data, 60)));
    data_files.insert("clips_in_library".into(), count_entries(&data.join("library.json")));
    data_files.insert("projects".into(), count_entries(&data.join("projects.json")));
    data_files.insert("games_seen".into(), count_entries(&data.join("games.json")));

    let system = json!({
        "windows": windows_version(),
        "hardware": hardware(),
        "webview2": webview2_version(),
        "disks": disks,
    });

    // The settings are scrubbed by name first; then every string of the whole report goes through the text rules.
    let unscrubbed = json!({
        "schema": 1,
        "tool": format!("glint-doctor {}", env!("CARGO_PKG_VERSION")),
        "created_unix": now_secs(),
        "glint_version": inst.version,
        "description": description.trim().chars().take(2000).collect::<String>(),
        "glint": inst.info,
        "system": system,
        "settings": if settings_raw.is_null() { json!("settings.json is missing or not readable") } else { scrub.settings(&settings_raw) },
        "ffmpeg": ffmpeg(&settings_raw, inst.dir.as_deref()),
        "processes": processes(),
        "crashes": crash_events(),
        "data": Value::Object(data_files),
        "log_bytes_total": log_raw.len(),
        "log_faults": fault_lines(&log_raw),
        "log_tail": tail(&log_raw, LOG_LINES, LOG_BYTES),
    });
    let mut bundle = scrub.json(&unscrubbed);
    fit(&mut bundle);
    bundle
}

/// Keep the upload under `MAX_BUNDLE`: drop the oldest lines of the log tail until it fits.
fn fit(bundle: &mut Value) {
    let size = |b: &Value| serde_json::to_string(b).map(|s| s.len()).unwrap_or(0);
    let mut guard = 0;
    while size(bundle) > MAX_BUNDLE && guard < 40 {
        guard += 1;
        let over = size(bundle) - MAX_BUNDLE;
        let log = bundle["log_tail"].as_str().unwrap_or("").to_string();
        if log.is_empty() {
            let faults = bundle["log_faults"].as_str().unwrap_or("").to_string();
            if faults.is_empty() {
                break;
            }
            bundle["log_faults"] = json!(tail(&faults, usize::MAX, faults.len().saturating_sub(over + 1024)));
            continue;
        }
        bundle["log_tail"] = json!(tail(&log, usize::MAX, log.len().saturating_sub(over + 1024)));
    }
}

/// The report as plain text, for the user to read before sending.
pub fn preview(bundle: &Value) -> String {
    let mut out = String::from("This is everything Glint Doctor will send. Nothing else leaves this PC.\r\n\r\n");
    let Some(map) = bundle.as_object() else { return out };
    for (key, value) in map {
        out.push_str(&format!("==== {key} ====\r\n"));
        match value {
            Value::String(s) => out.push_str(&s.replace('\n', "\r\n")),
            other => out.push_str(&serde_json::to_string_pretty(other).unwrap_or_default().replace('\n', "\r\n")),
        }
        out.push_str("\r\n\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_cuts_at_lines() {
        assert_eq!(tail("a\nbb\nccc", 2, 100), "bb\nccc");
        assert_eq!(tail("a\nbb\nccc", 3, 5), "ccc");
        assert_eq!(tail("", 3, 5), "");
    }

    #[test]
    fn faults_are_counted_once() {
        let log = "2026-11-18 10:00:00.000 capture: started\n2026-11-18 10:00:01.000 export: ffmpeg failed (exit code 1)\n2026-11-18 10:00:02.000 update: GitHub could not be reached\n2026-11-18 10:05:01.000 export: ffmpeg failed (exit code 1)";
        let f = fault_lines(log);
        assert_eq!(f.lines().count(), 2, "{f}");
        assert!(f.lines().last().unwrap().starts_with("2026-11-18 10:05:01.000 export: ffmpeg failed") && f.contains("[2 times"), "{f}");
    }

    #[test]
    fn the_bundle_is_cut_to_fit() {
        let mut b = json!({ "log_tail": "line of the log\n".repeat(60_000), "log_faults": "x" });
        fit(&mut b);
        assert!(serde_json::to_string(&b).unwrap().len() <= MAX_BUNDLE);
        assert!(b["log_tail"].as_str().unwrap().ends_with("line of the log"));
    }
}
