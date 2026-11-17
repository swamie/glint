//! Talking to the running Glint through its local plugin API (src-tauri/src/plugins.rs in Glint).
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

/// Glint's data folder: `GLINT_DATA_DIR`, else `%APPDATA%\Glint`.
pub fn data_dir() -> PathBuf {
    match std::env::var_os("GLINT_DATA_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Glint"),
    }
}

/// Glint's clips folder from its settings (empty if unknown).
pub fn clips_dir() -> String {
    std::fs::read_to_string(data_dir().join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["clips_dir"].as_str().map(str::to_string))
        .unwrap_or_default()
}

const NOT_RUNNING: &str = "Glint is not running, or the Claude plugin is not installed in it. Start Glint (and install the plugin with install.ps1), then try again.";

/// Call `method` in Glint (the plugin script answers "claude.*" methods). `secs`: how long to wait.
pub fn call(method: &str, mut params: Value, secs: u64) -> Result<Value, String> {
    let info = std::fs::read_to_string(data_dir().join("plugins").join("api.json")).map_err(|_| NOT_RUNNING.to_string())?;
    let info: Value = serde_json::from_str(&info).map_err(|_| NOT_RUNNING.to_string())?;
    let port = info["port"].as_u64().ok_or(NOT_RUNNING)?;
    let token = info["token"].as_str().ok_or(NOT_RUNNING)?;
    let stream = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_secs(3)).map_err(|_| NOT_RUNNING.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(secs + 15))).ok();
    let mut w = stream.try_clone().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    writeln!(w, "{}", json!({ "token": token })).map_err(|e| e.to_string())?;
    r.read_line(&mut line).map_err(|_| NOT_RUNNING.to_string())?;
    let hello: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    if hello["ok"] != json!(true) {
        return Err("Glint refused the connection (restart Glint so it writes a new api.json)".into());
    }
    if params.is_null() {
        params = json!({});
    }
    if let Some(o) = params.as_object_mut() {
        o.insert("timeout_ms".into(), json!(secs * 1000));
    }
    writeln!(w, "{}", json!({ "id": 1, "method": method, "params": params })).map_err(|e| e.to_string())?;
    line.clear();
    r.read_line(&mut line).map_err(|e| format!("no answer from Glint: {e}"))?;
    let ans: Value = serde_json::from_str(&line).map_err(|e| format!("Glint answered something unreadable: {e}"))?;
    if let Some(e) = ans.get("error") {
        return Err(e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string()));
    }
    Ok(ans.get("result").cloned().unwrap_or(Value::Null))
}

/// A "claude.*" method of the plugin script.
pub fn editor(method: &str, params: Value) -> Result<Value, String> {
    call(&format!("claude.{method}"), params, 120)
}
