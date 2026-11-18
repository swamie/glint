//! The two requests Glint Doctor makes, both to the developer's endpoint
//! (server/doctor) and both with curl.exe, which is part of Windows 10 1803 and
//! newer (Glint itself sends bug reports the same way, bugreport.rs `post`):
//!   POST <endpoint>/v1/submit            the diagnostics, with the support code
//!   GET  <endpoint>/v1/status/<ticket>   has the report been written yet
//! The support code and the body are handed to curl in files, not on its
//! command line, so no other program on the PC can read them from the process
//! list, and a profile path with spaces or non-English letters is never quoted.
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The endpoint this copy was built for (tools/glint-doctor/endpoint.txt).
/// `GLINT_DOCTOR_URL` replaces it at run time; the server's smoke test uses that.
pub fn endpoint() -> String {
    let built_in = include_str!("../endpoint.txt").trim();
    let url = match std::env::var("GLINT_DOCTOR_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => built_in.to_string(),
    };
    url.trim_end_matches('/').to_string()
}

/// A folder of our own for the request files, removed again by `Work::drop`.
pub struct Work(pub PathBuf);

impl Work {
    pub fn new() -> Result<Work, String> {
        let dir = std::env::temp_dir().join(format!("glint-doctor-{}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|e| format!("Could not make a working folder: {e}"))?;
        Ok(Work(dir))
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the server said: the HTTP status and the JSON it answered with.
pub struct Answer {
    pub status: u16,
    pub json: Value,
}

/// curl.exe on Windows; plain curl elsewhere, so the tool can be tried against `wrangler dev` on any PC.
const CURL: &str = if cfg!(windows) { "curl.exe" } else { "curl" };

/// A ticket the server gave: 24 lowercase letters and digits. Anything else is not put in a URL.
pub fn is_ticket(t: &str) -> bool {
    t.len() == 24 && t.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

fn curl(dir: &Path, args: &[&str], max_time: &str) -> Result<Answer, String> {
    let out = Command::new(CURL)
        .current_dir(dir)
        .args(["-sS", "--max-time", max_time, "-A", concat!("glint-doctor/", env!("CARGO_PKG_VERSION")), "-H", "Expect:", "-o", "-", "-w", "\n%{http_code}"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run curl.exe ({e}). It is part of Windows 10 version 1803 and newer."))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_str()));
    let status: u16 = code.trim().parse().unwrap_or(0);
    if status == 0 {
        let why = String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("no answer").trim().to_string();
        return Err(format!("Could not reach the developer's server. Check the internet connection and try again. ({why})"));
    }
    Ok(Answer { status, json: serde_json::from_str(body.trim()).unwrap_or(Value::Null) })
}

/// Send the diagnostics. Ok(ticket) when the server took them (and used up the code).
pub fn submit(work: &Work, code: &str, bundle: &Value) -> Result<String, String> {
    let body = serde_json::to_string(bundle).map_err(|e| format!("Could not prepare the report: {e}"))?;
    fs::write(work.0.join("body.json"), body).map_err(|e| format!("Could not prepare the report: {e}"))?;
    fs::write(work.0.join("headers.txt"), format!("Authorization: Bearer {code}\r\nContent-Type: application/json\r\n")).map_err(|e| format!("Could not prepare the report: {e}"))?;
    let url = format!("{}/v1/submit", endpoint());
    let result = curl(&work.0, &["-X", "POST", "-H", "@headers.txt", "--data-binary", "@body.json", &url], "60");
    let _ = fs::remove_file(work.0.join("headers.txt"));
    let _ = fs::remove_file(work.0.join("body.json"));
    let answer = result?;
    let said = answer.json["error"].as_str().unwrap_or("").to_string();
    match answer.status {
        200..=299 => answer.json["ticket"].as_str().filter(|t| is_ticket(t)).map(str::to_string).ok_or_else(|| "The server took the report but answered in a way this tool does not expect. Tell the developer; the report may well have arrived.".to_string()),
        401 | 403 | 410 => Err(if said.is_empty() { "The support code was not accepted.".into() } else { said }),
        413 => Err("The report was too large for the server. Tell the developer.".into()),
        429 => Err(if said.is_empty() { "Too many tries. Wait a while and try again.".into() } else { said }),
        s => Err(format!("The server could not take the report (HTTP {s}{}). Try again later; if it then says the code was used, the report did arrive.", if said.is_empty() { String::new() } else { format!(": {said}") })),
    }
}

/// Ask how far the report is. None when the server could not be asked.
pub fn status(work: &Work, ticket: &str) -> Option<Value> {
    if !is_ticket(ticket) {
        return None;
    }
    let url = format!("{}/v1/status/{ticket}", endpoint());
    let answer = curl(&work.0, &[&url], "15").ok()?;
    (answer.status == 200).then_some(answer.json)
}
