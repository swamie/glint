//! Plugins: optional add-ons that live OUTSIDE Glint's own build, and the marketplace they come from.
//!
//! Off unless one is installed. Glint looks in `<data_dir>\plugins\`; each sub-folder with a
//! `plugin.json` is a plugin, switched off while the folder also holds a file named `disabled`.
//! With no plugin installed nothing here runs: no script is loaded, no port is opened and nothing
//! is asked of the network (the marketplace is only fetched when its page is opened).
//!
//! `plugin.json`:
//!   { "id": "stream", "name": "Stream", "version": "0.1.0",
//!     "description": "...", "author": "Glint developers",
//!     "min_glint": "0.1.53",  // optional: the oldest Glint it works with
//!     "ui": "stream.js",      // optional: a script run in the main window (ui/plugins.js gives it `Glint.register`)
//!     "css": "stream.css",    // optional: a stylesheet for it
//!     "api": true,            // optional: the plugin's own program talks to Glint over the local API
//!     "actions": [            // optional: actions a keybind can carry ("plugin:<id>:<action id>")
//!       { "id": "toggle", "label": "Start / stop streaming",
//!         "process": "live",                    // optional: the keybind starts / stops this process (below)
//!         "started": "Stream started", "stopped": "Stream stopped" } ] }
//!
//! What a plugin gets from the backend:
//!   - its own settings: `Settings::plugins[<id>]`, saved with Glint's settings, never looked into;
//!   - processes: a plugin may run Glint's ffmpeg with its own arguments (`plugin_proc_start`),
//!     optionally fed Glint's live desktop + microphone audio on stdin. The arguments a keybind
//!     starts a process with are the ones the plugin's script last saved in its settings:
//!     `plugins[<id>].procs[<name>] = { "args": [...], "audio": bool }`. So a keybind works while
//!     the window is closed (the window, and with it every plugin script, is gone in the tray);
//!   - the local API (PR #2), only when an installed plugin asks for it: see `start`.
//!
//! The marketplace: `REGISTRY/index.json` lists the official plugins and, for each, its files with
//! their size and SHA-256. Installing downloads exactly those files from `REGISTRY/official/<id>/`
//! (the address is built here, never taken from the index), checks each against the index, and only
//! then puts the folder in place. Uses curl.exe and certutil.exe, like update.rs. Nothing about the
//! user or the PC is sent.
//!
//! The local API: a TCP port on 127.0.0.1, its number and a random token written to
//! `<data_dir>\plugins\api.json` ({ port, token, pid, version }). One JSON object per line. The
//! first line a client sends is { "token": "..." }; a wrong token closes the connection. Then each
//! line is a request { "id", "method", "params" } and each answer { "id", "result" } or
//! { "id", "error" }.
//!   - "glint.hello" is answered here: { version, window_open, ready }.
//!   - Every other method is handed to the main window as the event "plugin-rpc" { rid, method,
//!     params }; a plugin's script answers with the command `plugin_reply` (rid, ok, value). The
//!     window is opened first if it is closed. `params.timeout_ms` (1 s .. 1 h, default 2 min)
//!     limits the wait.
use crate::core::Core;
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, Webview};

type St<'a> = State<'a, Arc<Core>>;

/// Where the official plugins are published: the `plugins` folder of Glint's own repository.
pub const REGISTRY: &str = "https://raw.githubusercontent.com/swamie/glint/main/plugins";

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Action {
    pub id: String,
    pub label: String,
    /// A process of this plugin the action starts or stops ("" = the action is the script's business).
    pub process: String,
    /// The overlay's words when the action started / stopped its process.
    pub started: String,
    pub stopped: String,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub min_glint: String,
    pub ui: String,
    pub css: String,
    pub api: bool,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug)]
pub struct Plugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// No `disabled` file in its folder.
    pub enabled: bool,
    /// Installed from the marketplace (its folder holds the receipt the install wrote).
    pub official: bool,
}

impl Plugin {
    /// "" when this Glint can run it, else why not.
    fn problem(&self) -> String {
        let need = &self.manifest.min_glint;
        if !need.is_empty() && crate::update::newer(need, env!("CARGO_PKG_VERSION")) {
            return format!("Needs Glint {need} or newer");
        }
        String::new()
    }
    fn runs(&self) -> bool {
        self.enabled && self.problem().is_empty()
    }
}

const RECEIPT: &str = ".official.json";

pub fn plugins_dir() -> PathBuf {
    util::data_dir().join("plugins")
}

/// A plugin's id is also its folder's name: lower-case letters, digits and dashes only.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-')
}

/// Every plugin on disk, switched on or off (sorted by folder name). Reads the disk every time:
/// cheap, and the folder is the only record there is.
pub fn all() -> Vec<Plugin> {
    let Ok(rd) = fs::read_dir(plugins_dir()) else { return Vec::new() };
    let mut out: Vec<Plugin> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.is_dir())
        .filter_map(|dir| {
            let text = fs::read_to_string(dir.join("plugin.json")).ok()?;
            let mut manifest: Manifest = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    util::log(&format!("plugins: {} has an unreadable plugin.json: {e}", dir.display()));
                    return None;
                }
            };
            // The folder's name is the id: a manifest cannot claim another plugin's.
            manifest.id = dir.file_name()?.to_string_lossy().into_owned();
            if manifest.name.is_empty() {
                manifest.name = manifest.id.clone();
            }
            let enabled = !dir.join("disabled").exists();
            let official = dir.join(RECEIPT).is_file();
            Some(Plugin { dir, manifest, enabled, official })
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// The plugins that run: switched on, and made for this Glint or an older one.
pub fn installed() -> Vec<Plugin> {
    all().into_iter().filter(Plugin::runs).collect()
}

fn find(id: &str) -> Option<Plugin> {
    all().into_iter().find(|p| p.manifest.id == id)
}

/* ---------- scripts in the main window ---------- */

static READY: AtomicBool = AtomicBool::new(false);
/// (id, version) of the plugins the main window loaded last (`plugins_boot`). None until it asked.
static LOADED: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);

/// Tauri's page-load hook: a page of the main window started, so its plugin scripts are gone.
pub fn on_page_load<R: Runtime>(webview: &Webview<R>, finished: bool) {
    if webview.label() == "main" && !finished {
        READY.store(false, Ordering::SeqCst);
    }
}

/// A file of a plugin, by the name its manifest gives. None when it is not inside the plugin's folder.
fn read_inside(p: &Plugin, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let path = p.dir.join(name);
    let inside = path.canonicalize().ok().zip(p.dir.canonicalize().ok()).is_some_and(|(f, d)| f.starts_with(d));
    if !inside {
        util::log(&format!("plugins: {} names a file that is not in its folder: {name}", p.manifest.id));
        return None;
    }
    fs::read_to_string(&path).ok()
}

/// The scripts and stylesheets of the plugins that run, for the main window to load (ui/plugins.js,
/// once per page load). An empty list with none installed.
#[tauri::command(async)]
pub fn plugins_boot() -> Vec<Value> {
    let list = installed();
    *LOADED.lock().unwrap() = Some(list.iter().map(|p| (p.manifest.id.clone(), p.manifest.version.clone())).collect());
    list.iter()
        .map(|p| {
            let m = &p.manifest;
            let js = read_inside(p, &m.ui).unwrap_or_default();
            if !js.is_empty() {
                util::log(&format!("plugins: loading {} {}", m.id, m.version));
            }
            json!({ "id": m.id, "name": m.name, "version": m.version, "author": m.author, "js": js, "css": read_inside(p, &m.css).unwrap_or_default(), "actions": m.actions })
        })
        .collect()
}

fn state_json() -> Value {
    let list = all();
    let now: Vec<(String, String)> = list.iter().filter(|p| p.runs()).map(|p| (p.manifest.id.clone(), p.manifest.version.clone())).collect();
    let loaded = LOADED.lock().unwrap().clone();
    let installed: Vec<Value> = list
        .iter()
        .map(|p| {
            let m = &p.manifest;
            json!({ "id": m.id, "name": m.name, "version": m.version, "description": m.description, "author": m.author, "enabled": p.enabled, "official": p.official, "problem": p.problem(), "actions": m.actions })
        })
        .collect();
    json!({ "installed": installed, "restart": loaded.is_some_and(|l| l != now), "glint": env!("CARGO_PKG_VERSION") })
}

/// What is installed, and whether a restart is owed (the plugins that run are not the ones loaded).
#[tauri::command(async)]
pub fn plugins_state() -> Value {
    state_json()
}

/* ---------- the marketplace ---------- */

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
struct FileRef {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
struct Listing {
    id: String,
    name: String,
    description: String,
    author: String,
    version: String,
    min_glint: String,
    /// The `d` of one SVG path on a 24 x 24 grid, drawn as a line icon.
    icon: String,
    /// A short label beside the name, such as "Preview".
    tag: String,
    files: Vec<FileRef>,
}

#[derive(Deserialize, Serialize, Default, Clone, Debug)]
#[serde(default)]
struct Index {
    schema: u32,
    plugins: Vec<Listing>,
}

fn market_cache() -> PathBuf {
    plugins_dir().join("market.json")
}

fn fetch_index() -> Result<Index, String> {
    let out = crate::update::curl().args(["--max-time", "20"]).arg(format!("{REGISTRY}/index.json")).stderr(Stdio::piped()).output().map_err(|e| format!("Could not run curl.exe: {e}"))?;
    if !out.status.success() {
        return Err(format!("Could not reach the plugin list: {}", util::last_lines(&out.stderr, 1)));
    }
    let index: Index = serde_json::from_slice(&out.stdout).map_err(|e| format!("The plugin list could not be read: {e}"))?;
    if index.schema > 1 {
        return Err("The plugin list is made for a newer Glint. Update Glint first.".into());
    }
    let _ = fs::create_dir_all(plugins_dir());
    let _ = fs::write(market_cache(), &out.stdout);
    Ok(index)
}

/// The official plugins. When the network fails, the list as last seen, with `error` set.
#[tauri::command(async)]
pub fn plugins_market() -> Value {
    let public = |i: &Index| -> Vec<Value> {
        i.plugins
            .iter()
            .filter(|l| valid_id(&l.id))
            .map(|l| json!({ "id": l.id, "name": l.name, "description": l.description, "author": l.author, "version": l.version, "min_glint": l.min_glint, "icon": l.icon, "tag": l.tag,
                "size": l.files.iter().map(|f| f.size).sum::<u64>(), "too_new": !l.min_glint.is_empty() && crate::update::newer(&l.min_glint, env!("CARGO_PKG_VERSION")) }))
            .collect()
    };
    match fetch_index() {
        Ok(i) => json!({ "plugins": public(&i), "error": "", "cached": false }),
        Err(error) => {
            let old: Index = fs::read_to_string(market_cache()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            json!({ "plugins": public(&old), "error": error, "cached": true })
        }
    }
}

/// A file name the index may carry: a short relative path of plain names, with a known ending.
fn safe_rel(path: &str) -> bool {
    const ENDINGS: [&str; 8] = [".js", ".css", ".json", ".svg", ".png", ".md", ".txt", ".html"];
    let parts: Vec<&str> = path.split('/').collect();
    !path.is_empty()
        && path.len() <= 120
        && parts.len() <= 4
        && parts.iter().all(|s| !s.is_empty() && !s.starts_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
        && ENDINGS.iter().any(|e| path.ends_with(e))
}

/// Download, check and install the official plugin `id` (also over an older copy of it, which
/// keeps being switched on or off as it was). Progress comes as "plugin-progress" { id, pct }.
#[tauri::command(async)]
pub fn plugin_install(app: AppHandle, id: String) -> Result<Value, String> {
    if !valid_id(&id) {
        return Err("That is not a plugin's name".into());
    }
    let progress = |pct: f32| {
        let _ = app.emit("plugin-progress", json!({ "id": id, "pct": pct }));
    };
    progress(2.0);
    let index = fetch_index()?;
    let listing = index.plugins.iter().find(|l| l.id == id).ok_or("This plugin is no longer in the marketplace")?;
    if !listing.min_glint.is_empty() && crate::update::newer(&listing.min_glint, env!("CARGO_PKG_VERSION")) {
        return Err(format!("This plugin needs Glint {} or newer. Update Glint first.", listing.min_glint));
    }
    let total: u64 = listing.files.iter().map(|f| f.size).sum();
    if listing.files.is_empty() || listing.files.len() > 200 || total > 20_000_000 || !listing.files.iter().any(|f| f.path == "plugin.json") {
        return Err("The marketplace entry of this plugin is not valid".into());
    }
    if let Some(bad) = listing.files.iter().find(|f| !safe_rel(&f.path) || f.sha256.len() != 64 || f.size == 0) {
        return Err(format!("The marketplace entry of this plugin names a file Glint will not install: {}", bad.path));
    }

    let work = util::tmp_dir().join(format!("plugin-{}", util::new_id()));
    let fresh = work.join("new");
    fs::create_dir_all(&fresh).map_err(|e| format!("Cannot create a temporary folder: {e}"))?;
    let result = (|| -> Result<(), String> {
        let mut done = 0u64;
        for f in &listing.files {
            let dest = fresh.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("Cannot create a temporary folder: {e}"))?;
            }
            // The address is built here: the index only names files inside the plugin's own folder.
            let url = format!("{REGISTRY}/official/{id}/{}", f.path);
            let out = crate::update::curl().args(["--max-time", "120", "-o"]).arg(&dest).arg(&url).stderr(Stdio::piped()).output().map_err(|e| format!("Could not run curl.exe: {e}"))?;
            if !out.status.success() || !dest.is_file() {
                return Err(format!("Download failed: {}", util::last_lines(&out.stderr, 1)));
            }
            let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            let got = crate::update::sha256(&dest).ok_or("The download could not be checked (certutil.exe did not run), so it was not installed")?;
            if size != f.size || got != f.sha256.to_lowercase() {
                // The list and the files are published together, but a cache may hand out one before the other.
                return Err("The download does not match the marketplace's checksum, so it was not installed. If the plugin was just updated, try again in a few minutes.".into());
            }
            done += f.size;
            progress(5.0 + 85.0 * done as f32 / total.max(1) as f32);
        }
        let manifest: Manifest = fs::read_to_string(fresh.join("plugin.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("The plugin's plugin.json could not be read")?;
        if manifest.id != id || manifest.version != listing.version {
            return Err("The plugin's files do not match its marketplace entry".into());
        }
        let receipt = json!({ "version": listing.version, "installed": util::unix_ms(), "source": REGISTRY, "files": listing.files });
        fs::write(fresh.join(RECEIPT), receipt.to_string()).map_err(|e| format!("Could not write the plugin's files: {e}"))?;

        // In place of an older copy, which keeps its on / off state.
        let target = plugins_dir().join(&id);
        let was_off = target.join("disabled").exists();
        proc_stop_all(Some(&id));
        fs::create_dir_all(plugins_dir()).map_err(|e| format!("Cannot create the plugins folder: {e}"))?;
        let old = work.join("old");
        if target.exists() {
            fs::rename(&target, &old).or_else(|_| fs::remove_dir_all(&target)).map_err(|e| format!("Could not replace the installed plugin: {e}"))?;
        }
        if let Err(e) = fs::rename(&fresh, &target) {
            if old.exists() {
                let _ = fs::rename(&old, &target); // the copy that was there goes back
            }
            return Err(format!("Could not put the plugin in place: {e}"));
        }
        if was_off {
            let _ = fs::write(target.join("disabled"), "");
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result?;
    progress(100.0);
    util::log(&format!("plugins: installed {id} {} from the marketplace", listing.version));
    Ok(state_json())
}

/// Switch an installed plugin on or off (the file `disabled` in its folder). Off also ends its processes.
#[tauri::command(async)]
pub fn plugin_set_enabled(id: String, on: bool) -> Result<Value, String> {
    let p = find(&id).ok_or("This plugin is not installed")?;
    let flag = p.dir.join("disabled");
    if on {
        if flag.exists() {
            fs::remove_file(&flag).map_err(|e| format!("Could not switch the plugin on: {e}"))?;
        }
    } else {
        fs::write(&flag, "").map_err(|e| format!("Could not switch the plugin off: {e}"))?;
        proc_stop_all(Some(&id));
    }
    util::log(&format!("plugins: {id} switched {}", if on { "on" } else { "off" }));
    Ok(state_json())
}

/// Remove an installed plugin's folder. Its settings stay in the settings file, for a later install.
#[tauri::command(async)]
pub fn plugin_uninstall(id: String) -> Result<Value, String> {
    if !valid_id(&id) {
        return Err("That is not a plugin's name".into());
    }
    let p = find(&id).ok_or("This plugin is not installed")?;
    proc_stop_all(Some(&id));
    fs::remove_dir_all(&p.dir).map_err(|e| format!("Could not remove the plugin: {e}"))?;
    util::log(&format!("plugins: removed {id}"));
    Ok(state_json())
}

/* ---------- a plugin's own files ---------- */

/// What a plugin may keep a copy of: pictures.
const KEEP_EXT: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const KEEP_MAX: u64 = 64 * 1024 * 1024;

/// Where a plugin's kept files live. Not inside its own folder, which an update replaces.
fn files_dir(id: &str) -> PathBuf {
    util::data_dir().join("plugin-files").join(id)
}

/// Copy a picture the user chose (dropped on the plugin's page, say) into the plugin's files, so
/// that it is still there after the original is moved or deleted: { path, name, size }.
#[tauri::command(async)]
pub fn plugin_file_keep(plugin: String, path: String) -> Result<Value, String> {
    if !valid_id(&plugin) || !installed().iter().any(|p| p.manifest.id == plugin) {
        return Err("This plugin is not installed or is switched off".into());
    }
    let src = PathBuf::from(&path);
    let meta = fs::metadata(&src).map_err(|_| "That file could not be read")?;
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !meta.is_file() || !KEEP_EXT.contains(&ext.as_str()) {
        return Err("Only pictures can be added: PNG, JPEG, GIF, WebP or BMP".into());
    }
    if meta.len() == 0 || meta.len() > KEEP_MAX {
        return Err(format!("That picture is too large (the limit is {} MB)", KEEP_MAX / 1024 / 1024));
    }
    let shown = src.file_name().and_then(|n| n.to_str()).unwrap_or("picture").to_string();
    let stem: String = src.file_stem().and_then(|n| n.to_str()).unwrap_or("picture").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
    let dir = files_dir(&plugin);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not keep the picture: {e}"))?;
    let dest = dir.join(format!("{:x}-{stem}.{ext}", util::unix_ms()));
    fs::copy(&src, &dest).map_err(|e| format!("Could not keep the picture: {e}"))?;
    Ok(json!({ "path": dest.to_string_lossy(), "name": shown, "size": meta.len() }))
}

/// Delete a file `plugin_file_keep` made. Anything that is not directly in that plugin's files is left alone.
#[tauri::command(async)]
pub fn plugin_file_drop(plugin: String, path: String) -> bool {
    if !valid_id(&plugin) {
        return false;
    }
    let p = PathBuf::from(&path);
    p.parent() == Some(files_dir(&plugin).as_path()) && p.is_file() && fs::remove_file(&p).is_ok()
}

/// Restart Glint so that a plugin just installed, removed or switched takes effect. The new copy
/// shows its window, and clips or stays paused as this one did.
#[tauri::command(async)]
pub fn plugins_restart(app: AppHandle, core: St) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    proc_stop_all(None);
    util::log("plugins: restarting Glint");
    crate::update::relaunch(&exe, false, Some(core.status().recording));
    app.exit(0);
    Ok(())
}

/* ---------- processes ---------- */

struct Proc {
    plugin: String,
    name: String,
    pid: u32,
    started: u64,
    child: Mutex<Option<Child>>,
    /// Stopped on purpose (not a failure).
    stopping: AtomicBool,
    running: AtomicBool,
    /// The last lines ffmpeg wrote.
    log: Mutex<VecDeque<String>>,
    /// How it ended: "" while running or when stopped on purpose, else the reason.
    error: Mutex<String>,
}

impl Proc {
    fn json(&self) -> Value {
        let log: Vec<String> = self.log.lock().unwrap().iter().cloned().collect();
        json!({ "plugin": self.plugin, "name": self.name, "running": self.running.load(Ordering::SeqCst), "pid": self.pid, "started": self.started, "error": *self.error.lock().unwrap(), "log": log })
    }
}

static PROCS: OnceLock<Mutex<HashMap<String, Arc<Proc>>>> = OnceLock::new();

fn procs() -> &'static Mutex<HashMap<String, Arc<Proc>>> {
    PROCS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn proc_key(plugin: &str, name: &str) -> String {
    format!("{plugin}/{name}")
}

fn proc_get(plugin: &str, name: &str) -> Option<Arc<Proc>> {
    procs().lock().unwrap().get(&proc_key(plugin, name)).cloned()
}

fn proc_is_running(plugin: &str, name: &str) -> bool {
    proc_get(plugin, name).is_some_and(|p| p.running.load(Ordering::SeqCst))
}

/// How far behind the clock the audio handed to a process is: time for the capture to deliver it.
const AUDIO_DELAY: f64 = 0.25;

/// Start `plugin`'s process `name`: Glint's ffmpeg with `args`. With `audio`, Glint's live desktop
/// and microphone audio (what is being captured for clips; silence while there is none) is written
/// to its stdin as s16le stereo, and "{audio_rate}" in `args` becomes its sample rate, so the
/// arguments carry `-f s16le -ar {audio_rate} -ac 2 -i pipe:0`.
/// `quiet`: its end is not announced with a toast (a short question to ffmpeg, such as its list of cameras, always "fails").
fn proc_start(core: &Arc<Core>, plugin: &str, name: &str, args: &[String], audio: bool, quiet: bool) -> Result<Arc<Proc>, String> {
    if !installed().iter().any(|p| p.manifest.id == plugin) {
        return Err("This plugin is not installed or is switched off".into());
    }
    if args.is_empty() || args.len() > 400 {
        return Err("The plugin gave no command to run".into());
    }
    if proc_is_running(plugin, name) {
        return Err("It is already running".into());
    }
    let ffmpeg = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found. Install ffmpeg or set its path in Settings.")?;
    let now = util::now_secs();
    let rate = core.rec.live_audio(now - 0.05, now).map(|p| p.rate).filter(|r| *r > 0).unwrap_or(48_000);
    let args: Vec<String> = args.iter().map(|a| a.replace("{audio_rate}", &rate.to_string())).collect();
    let mut child = util::hidden_cmd(&ffmpeg)
        .args(&args)
        .stdin(if audio { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    util::track_child(&child); // ends with Glint
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    // An address among the arguments may carry a secret (a stream key is the end of the address):
    // what ffmpeg says is kept, shown and logged with only the server's name left of it.
    let secrets: Vec<(String, String)> = args.iter().filter(|a| a.contains("://")).map(|a| (a.clone(), redact(a))).collect();
    let p = Arc::new(Proc {
        plugin: plugin.into(),
        name: name.into(),
        pid: child.id(),
        started: util::unix_ms(),
        child: Mutex::new(Some(child)),
        stopping: AtomicBool::new(false),
        running: AtomicBool::new(true),
        log: Mutex::new(VecDeque::new()),
        error: Mutex::new(String::new()),
    });
    procs().lock().unwrap().insert(proc_key(plugin, name), p.clone());
    util::log(&format!("plugins: {plugin} started its process {name} (pid {}{})", p.pid, if audio { format!(", live audio at {rate} Hz") } else { String::new() }));

    // Glint's live audio into its stdin, in step with the clock.
    if let (true, Some(mut stdin)) = (audio, stdin) {
        let (p2, rec) = (p.clone(), core.rec.clone());
        std::thread::spawn(move || {
            let frame = |t: f64| (t * rate as f64).round() as i64;
            let mut from = util::now_secs() - AUDIO_DELAY;
            while p2.running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                let to = util::now_secs() - AUDIO_DELAY;
                let frames = (frame(to) - frame(from)).max(0) as usize;
                if frames == 0 {
                    continue;
                }
                // Anything but audio at the rate ffmpeg was told (none captured, a device changed) is silence of the same length.
                let data = rec.live_audio(from, to).filter(|pcm| pcm.rate == rate && pcm.data.len() == frames * 2).map(|pcm| pcm.data).unwrap_or_else(|| vec![0i16; frames * 2]);
                let bytes: Vec<u8> = data.iter().flat_map(|s| s.to_le_bytes()).collect();
                if stdin.write_all(&bytes).is_err() {
                    break; // ffmpeg is gone
                }
                from = to;
            }
        });
    }

    // Its messages, and its end.
    let (p2, core2) = (p.clone(), core.clone());
    std::thread::spawn(move || {
        if let Some(mut err) = stderr {
            // ffmpeg ends its progress lines with a carriage return: both count as a line's end.
            let mut line: Vec<u8> = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    if b == b'\n' || b == b'\r' {
                        let mut text = String::from_utf8_lossy(&line).trim().to_string();
                        line.clear();
                        for (secret, shown) in &secrets {
                            text = text.replace(secret.as_str(), shown);
                        }
                        if !text.is_empty() {
                            let mut log = p2.log.lock().unwrap();
                            log.push_back(text);
                            while log.len() > 40 {
                                log.pop_front();
                            }
                        }
                    } else if line.len() < 2000 {
                        line.push(b);
                    }
                }
            }
        }
        let status = p2.child.lock().unwrap().take().and_then(|mut c| c.wait().ok());
        let on_purpose = p2.stopping.load(Ordering::SeqCst);
        if !on_purpose {
            // The last line that is not a progress line says why.
            let why = p2.log.lock().unwrap().iter().rev().find(|l| !l.starts_with("frame=") && !l.starts_with("size=")).cloned().unwrap_or_default();
            *p2.error.lock().unwrap() = if why.is_empty() { format!("ffmpeg ended ({})", status.map(|s| s.to_string()).unwrap_or_default()) } else { why };
        }
        p2.running.store(false, Ordering::SeqCst);
        util::log(&format!("plugins: {}'s process {} ended{}", p2.plugin, p2.name, if on_purpose { String::new() } else { format!(": {}", p2.error.lock().unwrap()) }));
        core2.emit("plugin-proc", p2.json());
        if !on_purpose && !quiet && core2.settings().toast {
            let name = find(&p2.plugin).map(|x| x.manifest.name).unwrap_or_else(|| p2.plugin.clone());
            let lang = core2.settings().language;
            let why = p2.error.lock().unwrap().clone();
            core2.show_toast(&format!("{name}: {}", crate::lang::tr(&lang, "Stopped")), &why);
        }
    });
    core.emit("plugin-proc", p.json());
    Ok(p)
}

/// "rtmp://live.example.tv/app/secret-key" -> "rtmp://live.example.tv/…".
fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?', '@']).next().unwrap_or("");
    format!("{scheme}://{host}/…")
}

fn proc_stop(plugin: &str, name: &str) -> bool {
    let Some(p) = proc_get(plugin, name) else { return false };
    if !p.running.load(Ordering::SeqCst) {
        return false;
    }
    p.stopping.store(true, Ordering::SeqCst);
    if let Some(c) = p.child.lock().unwrap().as_mut() {
        let _ = c.kill();
    }
    true
}

/// End every process of `plugin`, or of every plugin.
fn proc_stop_all(plugin: Option<&str>) {
    let list: Vec<Arc<Proc>> = procs().lock().unwrap().values().cloned().collect();
    for p in list.iter().filter(|p| plugin.is_none_or(|id| p.plugin == id)) {
        proc_stop(&p.plugin, &p.name);
    }
}

#[tauri::command(async)]
pub fn plugin_proc_start(core: St, plugin: String, name: String, args: Vec<String>, audio: Option<bool>, quiet: Option<bool>) -> Result<Value, String> {
    proc_start(core.inner(), &plugin, &name, &args, audio.unwrap_or(false), quiet.unwrap_or(false)).map(|p| p.json())
}

#[tauri::command(async)]
pub fn plugin_proc_stop(plugin: String, name: String) -> Value {
    proc_stop(&plugin, &name);
    plugin_proc_status(plugin, name)
}

/// { running, started, pid, error, log } of a plugin's process ({ running: false } when it never ran).
#[tauri::command(async)]
pub fn plugin_proc_status(plugin: String, name: String) -> Value {
    proc_get(&plugin, &name).map(|p| p.json()).unwrap_or_else(|| json!({ "plugin": plugin, "name": name, "running": false, "pid": 0, "started": 0, "error": "", "log": [] }))
}

/* ---------- keybinds ---------- */

/// A keybind whose action is "plugin:<id>:<action>" was pressed. With a `process` in the action's
/// manifest entry, that process is started (with the arguments the plugin's script last saved in
/// `plugins[<id>].procs[<process>]`) or stopped, and the overlay says which. The main window, when
/// it is open, also gets the event "plugin-action" { plugin, action } for the plugin's script.
pub fn fire(core: &Arc<Core>, action: &str) {
    let mut parts = action.splitn(3, ':');
    let (_, Some(id), Some(act)) = (parts.next(), parts.next(), parts.next()) else { return };
    let Some(p) = installed().into_iter().find(|p| p.manifest.id == id) else {
        return util::log(&format!("plugins: a keybind asked for {action}, but that plugin is not installed or is off"));
    };
    let Some(def) = p.manifest.actions.iter().find(|a| a.id == act) else { return };
    let s = core.settings();
    let toast = |title: &str, sub: &str| {
        if s.toast {
            core.show_toast(title, sub);
        }
    };
    if !def.process.is_empty() {
        if proc_stop(id, &def.process) {
            toast(if def.stopped.is_empty() { &p.manifest.name } else { &def.stopped }, "");
        } else {
            let saved = &s.plugins.get(id).cloned().unwrap_or(Value::Null)["procs"][&def.process];
            let args: Vec<String> = saved["args"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            if args.is_empty() {
                toast(&p.manifest.name, crate::lang::tr(&s.language, "Open Glint and set it up first"));
            } else {
                match proc_start(core, id, &def.process, &args, saved["audio"].as_bool().unwrap_or(false), false) {
                    Ok(_) => toast(if def.started.is_empty() { &p.manifest.name } else { &def.started }, ""),
                    Err(e) => toast(&p.manifest.name, &e),
                }
            }
        }
    }
    core.emit("plugin-action", json!({ "plugin": id, "action": act }));
}

/* ---------- the local API ---------- */

static PENDING: OnceLock<Mutex<HashMap<u64, Sender<Result<Value, String>>>>> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(1);

fn pending() -> &'static Mutex<HashMap<u64, Sender<Result<Value, String>>>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A plugin script is listening in the main window (it calls this once it has subscribed).
#[tauri::command]
pub fn plugin_ready() {
    READY.store(true, Ordering::SeqCst);
}

/// A plugin script's answer to "plugin-rpc" `rid`.
#[tauri::command]
pub fn plugin_reply(rid: u64, ok: bool, value: Value) {
    if let Some(tx) = pending().lock().unwrap().remove(&rid) {
        let _ = tx.send(if ok { Ok(value) } else { Err(value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())) });
    }
}

/// A random token for api.json: std's RandomState is seeded by the operating system.
fn token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|i| {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(util::unix_ms() ^ (std::process::id() as u64) << 20 ^ i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// Called once at start-up: log what is installed, and start the local API if an installed plugin asks for it.
pub fn start(app: AppHandle, show_main: fn(&AppHandle)) {
    let list = installed();
    if !list.is_empty() {
        util::log(&format!("plugins: {}", list.iter().map(|p| format!("{} {}", p.manifest.id, p.manifest.version)).collect::<Vec<_>>().join(", ")));
    }
    if !list.iter().any(|p| p.manifest.api) {
        let _ = fs::remove_file(plugins_dir().join("api.json")); // a stale one from an earlier run
        return;
    }
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => return util::log(&format!("plugins: the local API could not start: {e}")),
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let token = token();
    let info = json!({ "port": port, "token": token, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION") });
    if let Err(e) = fs::write(plugins_dir().join("api.json"), info.to_string()) {
        return util::log(&format!("plugins: api.json could not be written: {e}"));
    }
    util::log(&format!("plugins: local API on 127.0.0.1:{port} for {}", list.iter().filter(|p| p.manifest.api).map(|p| p.manifest.id.as_str()).collect::<Vec<_>>().join(", ")));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            let token = token.clone();
            std::thread::spawn(move || serve(stream, &app, &token, show_main));
        }
    });
}

fn serve(stream: TcpStream, app: &AppHandle, token: &str, show_main: fn(&AppHandle)) {
    let Ok(write_half) = stream.try_clone() else { return };
    let out = Mutex::new(write_half);
    let send = |v: Value| {
        let mut w = out.lock().unwrap();
        let _ = writeln!(w, "{v}");
        let _ = w.flush();
    };
    let mut lines = BufReader::new(stream).lines();
    let authed = lines.next().and_then(|l| l.ok()).and_then(|l| serde_json::from_str::<Value>(&l).ok()).is_some_and(|v| v["token"].as_str() == Some(token));
    if !authed {
        send(json!({ "error": "wrong token" }));
        return;
    }
    send(json!({ "ok": true }));
    // Requests are answered in order, one at a time per connection.
    for line in lines.map_while(Result::ok) {
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            send(json!({ "id": null, "error": "not JSON" }));
            continue;
        };
        let id = req["id"].clone();
        let method = req["method"].as_str().unwrap_or("").to_string();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let answer = if method == "glint.hello" {
            Ok(json!({ "version": env!("CARGO_PKG_VERSION"), "window_open": app.get_webview_window("main").is_some(), "ready": READY.load(Ordering::SeqCst) }))
        } else {
            call(app, &method, params, show_main)
        };
        send(match answer {
            Ok(result) => json!({ "id": id, "result": result }),
            Err(error) => json!({ "id": id, "error": error }),
        });
    }
}

/// Hand `method` to the plugin script in the main window and wait for its answer.
fn call(app: &AppHandle, method: &str, params: Value, show_main: fn(&AppHandle)) -> Result<Value, String> {
    let ms = params["timeout_ms"].as_u64().unwrap_or(120_000).clamp(1_000, 3_600_000);
    if !READY.load(Ordering::SeqCst) {
        if app.get_webview_window("main").is_none() {
            show_main(app);
        }
        let t0 = Instant::now();
        while !READY.load(Ordering::SeqCst) {
            if t0.elapsed() > Duration::from_secs(30) {
                return Err("Glint's window did not get ready in 30 seconds".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let rid = SEQ.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = channel();
    pending().lock().unwrap().insert(rid, tx);
    if let Err(e) = app.emit_to("main", "plugin-rpc", json!({ "rid": rid, "method": method, "params": params })) {
        pending().lock().unwrap().remove(&rid);
        return Err(format!("could not reach Glint's window: {e}"));
    }
    let answer = rx.recv_timeout(Duration::from_millis(ms));
    pending().lock().unwrap().remove(&rid);
    answer.unwrap_or_else(|_| Err(format!("no answer from Glint in {} s (is the window still open?)", ms / 1000)))
}
