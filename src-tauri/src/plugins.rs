//! Plugins: optional add-ons that live OUTSIDE Glint's own build.
//!
//! Off unless one is installed. Glint looks in `<data_dir>\plugins\`; each sub-folder with a
//! `plugin.json` is a plugin, unless the folder also holds a file named `disabled`. With no plugin
//! installed nothing here runs: no script is loaded and no port is opened.
//!
//! `plugin.json`:
//!   { "id": "claude-mcp", "name": "Claude (MCP)", "version": "0.1.0",
//!     "ui": "bridge.js",   // optional: a script loaded into the main window after each page load
//!     "api": true }        // optional: the plugin's own program talks to Glint over the local API
//!
//! The local API (only when an installed plugin asks for it): a TCP port on 127.0.0.1, its number
//! and a random token written to `<data_dir>\plugins\api.json` ({ port, token, pid, version }).
//! One JSON object per line. The first line a client sends is { "token": "..." }; a wrong token
//! closes the connection. Then each line is a request { "id", "method", "params" } and each answer
//! { "id", "result" } or { "id", "error" }.
//!   - "glint.hello" is answered here: { version, window_open, ready }.
//!   - Every other method is handed to the main window as the event "plugin-rpc" { rid, method,
//!     params }; a plugin's script answers with the command `plugin_reply` (rid, ok, value). The
//!     window is opened first if it is closed. `params.timeout_ms` (1 s .. 1 h, default 2 min)
//!     limits the wait.
//! Glint itself never acts on these methods: what they do is the plugin script's business.
use crate::util;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, Webview};

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub ui: String,
    pub api: bool,
}

#[derive(Clone, Debug)]
pub struct Plugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn plugins_dir() -> PathBuf {
    util::data_dir().join("plugins")
}

/// The installed, enabled plugins (sorted by folder name). Reads the disk every time: cheap, and a
/// plugin added or removed counts from the next page load.
pub fn installed() -> Vec<Plugin> {
    let Ok(rd) = fs::read_dir(plugins_dir()) else { return Vec::new() };
    let mut out: Vec<Plugin> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.is_dir() && !d.join("disabled").exists())
        .filter_map(|dir| {
            let text = fs::read_to_string(dir.join("plugin.json")).ok()?;
            let mut manifest: Manifest = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    util::log(&format!("plugins: {} has an unreadable plugin.json: {e}", dir.display()));
                    return None;
                }
            };
            if manifest.id.is_empty() {
                manifest.id = dir.file_name()?.to_string_lossy().into_owned();
            }
            Some(Plugin { dir, manifest })
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/* ---------- scripts in the main window ---------- */

static READY: AtomicBool = AtomicBool::new(false);

/// Tauri's page-load hook: a page of the main window started (its plugin scripts are gone) or
/// finished (load them). Nothing at all for other windows or with no plugin installed.
pub fn on_page_load<R: Runtime>(webview: &Webview<R>, finished: bool) {
    if webview.label() != "main" {
        return;
    }
    if !finished {
        READY.store(false, Ordering::SeqCst);
        return;
    }
    for p in installed() {
        if p.manifest.ui.is_empty() {
            continue;
        }
        let path = p.dir.join(&p.manifest.ui);
        // The script must stay inside its plugin's folder.
        let inside = path.canonicalize().ok().zip(p.dir.canonicalize().ok()).is_some_and(|(f, d)| f.starts_with(d));
        let Some(code) = fs::read_to_string(&path).ok().filter(|_| inside) else {
            util::log(&format!("plugins: {} names a script that is not in its folder: {}", p.manifest.id, p.manifest.ui));
            continue;
        };
        let id = serde_json::to_string(&p.manifest.id).unwrap_or_default();
        let wrapped = format!("(function(){{try{{\n{code}\n}}catch(e){{console.error('Glint plugin '+{id}+' failed to load',e);}}}})();");
        if let Err(e) = webview.eval(&wrapped) {
            util::log(&format!("plugins: {} could not be loaded: {e}", p.manifest.id));
        } else {
            util::log(&format!("plugins: loaded {} {}", p.manifest.id, p.manifest.version));
        }
    }
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

/// Start the local API if an installed plugin asks for it. Called once at start-up.
pub fn start(app: AppHandle, show_main: fn(&AppHandle)) {
    let list = installed();
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
