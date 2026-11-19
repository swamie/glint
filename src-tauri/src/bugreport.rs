//! "Report a bug": a message from the user, posted to the developer's Discord.
//!
//! The report goes to the Glint server (the share Worker's `/report` route,
//! server/share), which passes it on to the developers' Discord. The Discord
//! webhook and the ids to ping are secrets of that Worker; nothing about them
//! is in the program, so every build can send reports.
//!
//! What is sent, and only when the user presses Send: their text, the contact
//! they typed (if any), the `system` lines they were shown (Glint and Windows
//! versions, graphics card, CPU, capture settings) and, if they tick the box,
//! the last lines of `glint.log`. Every occurrence of the user's profile folder
//! is replaced by `~` first, so the Windows user name does not leave the PC.
//!
//! Network use: one HTTPS POST to clips.glintclip.com per report, with curl.exe
//! (it ships with Windows 10 and 11). `GLINT_REPORT_URL` replaces the address
//! at run time; the self-test points it at a listener on 127.0.0.1.
use crate::core::Core;
use crate::{util, win};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where reports and feedback are posted (`?kind=bug` or `?kind=feedback` is added).
const REPORT_URL: &str = "https://clips.glintclip.com/report";

/// Lines of glint.log a report carries.
const LOG_LINES: usize = 150;
const MIN_TEXT: usize = 10;
const MAX_TEXT: usize = 4000;
const MAX_CONTACT: usize = 200;
/// Seconds after a report was sent before the next one is accepted.
const PAUSE: Duration = Duration::from_secs(30);

static LAST_SENT: Mutex<Option<Instant>> = Mutex::new(None);

/// What the bug-report dialog shows before anything is sent.
#[derive(Serialize, Clone, Debug, Default)]
pub struct BugInfo {
    /// This build has somewhere to send reports to.
    pub configured: bool,
    /// Glint's version, e.g. "0.1.6".
    pub version: String,
    /// A few plain lines about this PC and the capture setup, sent with the report.
    pub system: String,
    /// The last 150 lines of glint.log, as they would be attached.
    pub log: String,
}

/// The report address: `GLINT_REPORT_URL` when set, else the Glint server.
fn endpoint() -> String {
    match std::env::var("GLINT_REPORT_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => REPORT_URL.to_string(),
    }
}

/// Replace every occurrence of the user's profile folder with `~`. Letter case
/// is ignored and `\`, `/` and doubled separators (as in JSON) all match.
pub fn sanitize(text: &str) -> String {
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    sanitize_with(text, &profile)
}

fn sanitize_with(text: &str, profile: &str) -> String {
    let pat: Vec<char> = profile.trim_end_matches(['\\', '/']).chars().collect();
    if pat.len() < 3 {
        return text.to_string();
    }
    let is_sep = |c: char| c == '\\' || c == '/';
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        // Try to match the profile path at i.
        let (mut j, mut k) = (i, 0);
        while k < pat.len() && j < chars.len() {
            if is_sep(pat[k]) {
                if !is_sep(chars[j]) {
                    break;
                }
                while j < chars.len() && is_sep(chars[j]) {
                    j += 1;
                }
            } else if same(pat[k], chars[j]) {
                j += 1;
            } else {
                break;
            }
            k += 1;
        }
        // Only a whole folder name: C:\Users\v must not eat the start of C:\Users\vera.
        let ends = chars.get(j).map_or(true, |c| !(c.is_alphanumeric() || *c == '_' || *c == '-'));
        if k == pat.len() && ends {
            out.push('~');
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn windows_version() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else { return "Windows (version unknown)".into() };
    let s = |name: &str| key.get_value::<String, _>(name).unwrap_or_default();
    let build = s("CurrentBuildNumber");
    let mut product = s("ProductName");
    // Windows 11 still calls itself "Windows 10" here; the build number tells them apart.
    if build.parse::<u32>().unwrap_or(0) >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    let ubr = key.get_value::<u32, _>("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    let display = s("DisplayVersion");
    let display = if display.is_empty() { String::new() } else { format!(" {display}") };
    format!("{}{display} (build {build}{ubr})", if product.is_empty() { "Windows".to_string() } else { product })
}

fn system_text(core: &Core) -> String {
    let s = core.settings();
    let st = core.status();
    let hw = win::hardware();
    let mut lines = vec![format!("Glint {}", env!("CARGO_PKG_VERSION")), windows_version()];
    let gpus: Vec<&str> = hw.gpus.iter().map(|g| g.name.as_str()).collect();
    lines.push(format!("Graphics: {}", if gpus.is_empty() { "none found".to_string() } else { gpus.join(", ") }));
    lines.push(format!("CPU: {}", if hw.cpu.is_empty() { "unknown" } else { &hw.cpu }));
    if st.recording {
        // Every buffered monitor: "monitor 1 2560x1440 at 60 fps (clips up to 30 s), monitor 2 ...".
        let mons: Vec<String> = st
            .captures
            .iter()
            .map(|c| format!("monitor {}{} {}x{} at {} fps (clips up to {} s{})", c.monitor + 1, if c.primary && st.captures.len() > 1 { " (main)" } else { "" }, c.width, c.height, c.fps, c.buffer_max.round(), if c.error.is_empty() { String::new() } else { format!(", error: {}", c.error) }))
            .collect();
        lines.push(format!("Capture: {}, encoder {}, method {}", mons.join("; "), st.encoder, st.method));
    } else {
        let height = if s.height == 0 { "native".to_string() } else { format!("{}p", s.height) };
        lines.push(format!("Capture: paused (settings: {height} at {} fps, encoder {}, method {})", s.fps, s.encoder, s.method));
    }
    if !st.error.is_empty() {
        lines.push(format!("Capture error: {}", st.error));
    }
    let buffer = match s.buffer.as_str() {
        "mixed" => format!("mixed ({} s in RAM, the rest on disk)", s.buffer_ram_secs),
        other => other.to_string(),
    };
    lines.push(format!("Replay buffer: {buffer}, clips up to {} s", s.max_clip_secs()));
    lines.push(format!("Interface language: {}", s.language));
    sanitize(&lines.join("\n"))
}

fn log_text() -> String {
    let raw = fs::read(util::data_dir().join("glint.log")).unwrap_or_default();
    let text = String::from_utf8_lossy(&raw);
    let lines: Vec<&str> = text.lines().collect();
    sanitize(&lines[lines.len().saturating_sub(LOG_LINES)..].join("\n"))
}

/// What the dialog shows: whether reports can be sent, and exactly what goes with one.
pub fn info(core: &Core) -> BugInfo {
    BugInfo { configured: !endpoint().is_empty(), version: env!("CARGO_PKG_VERSION").to_string(), system: system_text(core), log: log_text() }
}

/// At most `max` characters, with "…" at the end when something was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// The `payload_json` part of the post. The server adds the pings.
fn payload(text: &str, contact: &str, system: &str) -> serde_json::Value {
    let content = format!("Bug report · Glint {}", env!("CARGO_PKG_VERSION"));
    let mut fields = Vec::new();
    if !contact.is_empty() {
        fields.push(serde_json::json!({ "name": "Contact", "value": cut(contact, 1024), "inline": false }));
    }
    // A field holds 1024 characters; the code fence takes 8 of them.
    let system = system.replace("```", "'''");
    fields.push(serde_json::json!({ "name": "System", "value": format!("```\n{}\n```", cut(&system, 1024 - 8)), "inline": false }));
    serde_json::json!({
        "content": cut(&content, 2000),
        "allowed_mentions": { "parse": [] },
        "embeds": [{ "description": cut(text, 4096), "color": 0xf5b83d, "fields": fields }],
    })
}

/// A short reason, made safe to show: never the report address, never the profile folder.
fn scrub(msg: &str, url: &str) -> String {
    let mut m = msg.to_string();
    if !url.is_empty() {
        m = m.replace(url, "(the report address)");
    }
    sanitize(&m)
}

/// Send a bug report. `text` is the user's description, `contact` how to reach
/// them ("" = not given), `include_log` attaches the end of glint.log.
pub fn send(core: &Core, text: &str, contact: &str, include_log: bool) -> Result<(), String> {
    let text = text.trim();
    let contact = cut(contact.trim(), MAX_CONTACT);
    let n = text.chars().count();
    if n == 0 {
        return Err("Describe the problem before sending.".into());
    }
    if n < MIN_TEXT {
        return Err("Please describe the problem in a few more words.".into());
    }
    if n > MAX_TEXT {
        return Err(format!("The description is too long: {n} characters, at most {MAX_TEXT} fit."));
    }
    if LAST_SENT.lock().unwrap().is_some_and(|t| t.elapsed() < PAUSE) {
        return Err("Wait a moment before sending another report.".into());
    }
    let url = endpoint();
    let system = system_text(core);
    let body = serde_json::to_string(&payload(text, &contact, &system)).map_err(|e| format!("Could not prepare the report: {e}"))?;
    let log = include_log.then(|| {
        let log = log_text();
        if log.is_empty() { "(the log is empty)".to_string() } else { log }
    });
    let result = post(&url, "bug", &body, log.as_deref(), "report");
    match &result {
        Ok(()) => {
            *LAST_SENT.lock().unwrap() = Some(Instant::now());
            util::log(&format!("bug report: sent ({n} characters, log {})", if include_log { "attached" } else { "not attached" }));
        }
        Err(e) => util::log(&format!("bug report: not sent: {e}")),
    }
    result
}

/// Set by the self-test: from then on every post must go to its listener on
/// 127.0.0.1, whatever the address resolves to; anything else is refused
/// before curl runs.
static TEST_ONLY: AtomicBool = AtomicBool::new(false);

/// POST `body` as `payload_json` (and `log` as glint-log.txt when given) to
/// `url?kind=<prefix>` with curl.exe (`prefix` is "bug" or "fb"). `prefix` names the temporary files, `what` the thing in
/// error messages ("report", "feedback").
fn post(url: &str, prefix: &str, body: &str, log: Option<&str>, what: &str) -> Result<(), String> {
    if TEST_ONLY.load(Ordering::SeqCst) && !url.starts_with("http://127.0.0.1:") {
        return Err("Test run: nothing is sent anywhere but the test listener.".into());
    }
    // curl reads both parts from files named relative to its working folder,
    // so a profile path with spaces or non-English letters never has to pass
    // through its command line.
    let dir = util::tmp_dir();
    let id = util::new_id();
    let (json_name, log_name) = (format!("{prefix}-{id}.json"), format!("{prefix}-{id}-log.txt"));
    let result = (|| {
        fs::write(dir.join(&json_name), body).map_err(|e| format!("Could not prepare the {what}: {e}"))?;
        let mut cmd = util::hidden_cmd(Path::new("curl.exe"));
        cmd.current_dir(&dir)
            .args(["-sS", "--max-time", "20", "-A", "Glint", "-H", "Expect:", "-o", "-", "-w", "\n%{http_code}"])
            .arg("-F")
            .arg(format!("payload_json=<{json_name};type=application/json"));
        if let Some(log) = log {
            fs::write(dir.join(&log_name), log).map_err(|e| format!("Could not attach the log: {e}"))?;
            cmd.arg("-F").arg(format!("files[0]=@{log_name};filename=glint-log.txt;type=text/plain"));
        }
        let kind = if prefix == "fb" { "feedback" } else { "bug" };
        let out = cmd.arg(format!("{url}?kind={kind}")).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not run curl.exe: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let (answer, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_str()));
        let code: u16 = code.trim().parse().unwrap_or(0);
        if code == 0 {
            let why = util::last_lines(&out.stderr, 1);
            return Err(scrub(&format!("Could not reach Discord. Check the internet connection and try again. ({})", if why.is_empty() { "no answer" } else { &why }), url));
        }
        if !(200..=299).contains(&code) {
            // The server answers { "error": "..." } in plain words; Discord itself (a test listener) { "message": "..." }.
            let v = serde_json::from_str::<serde_json::Value>(answer.trim()).ok();
            let said = v.as_ref().and_then(|v| v["error"].as_str().map(|m| cut(m, 200)));
            let detail = v.as_ref().and_then(|v| v["message"].as_str().map(|m| cut(m, 120))).unwrap_or_default();
            let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
            let msg = match said {
                Some(m) if !m.is_empty() => m,
                _ if code == 429 => format!("Discord is busy right now (HTTP 429{detail}). Try again in a minute."),
                _ => format!("Discord did not accept the {what} (HTTP {code}{detail})."),
            };
            return Err(scrub(&msg, url));
        }
        Ok(())
    })();
    let _ = fs::remove_file(dir.join(&json_name));
    let _ = fs::remove_file(dir.join(&log_name));
    result
}

// ---- Feedback (the heart button): stars and a few words, to the same server.

const MAX_STARS: u8 = 5;
static LAST_FEEDBACK: Mutex<Option<Instant>> = Mutex::new(None);

/// What the feedback dialog needs before anything is sent.
#[derive(Serialize, Clone, Debug, Default)]
pub struct FeedbackInfo {
    /// This build has somewhere to send feedback to.
    pub configured: bool,
    /// Glint's version, e.g. "0.1.35".
    pub version: String,
    /// The interface language sent with the feedback ("en", "ko", "el").
    pub language: String,
}

pub fn feedback_info(core: &Core) -> FeedbackInfo {
    FeedbackInfo { configured: !endpoint().is_empty(), version: env!("CARGO_PKG_VERSION").to_string(), language: core.settings().language.clone() }
}

/// "★★★★☆ 4 / 5", or "No stars given" for 0.
fn stars_text(stars: u8) -> String {
    let n = stars.min(MAX_STARS) as usize;
    if n == 0 {
        return "No stars given".into();
    }
    format!("{}{} {n} / {MAX_STARS}", "★".repeat(n), "☆".repeat(MAX_STARS as usize - n))
}

/// The `payload_json` of a feedback post. It pings no one: no mention in
/// `content`, and `allowed_mentions` allows none, whatever the text holds.
fn feedback_payload(stars: u8, text: &str, contact: &str, language: &str) -> serde_json::Value {
    let version = env!("CARGO_PKG_VERSION");
    let mut fields = vec![serde_json::json!({ "name": "Stars", "value": stars_text(stars), "inline": true })];
    if !contact.is_empty() {
        fields.push(serde_json::json!({ "name": "Contact", "value": cut(contact, 1024), "inline": false }));
    }
    fields.push(serde_json::json!({ "name": "Glint", "value": format!("{version} · language {language}"), "inline": true }));
    let mut embed = serde_json::json!({ "title": "Feedback", "color": 0xe5527a, "fields": fields });
    if !text.is_empty() {
        embed["description"] = cut(text, 4096).into();
    }
    serde_json::json!({
        "content": format!("Feedback · Glint {version}"),
        "allowed_mentions": { "parse": [] },
        "embeds": [embed],
    })
}

/// Send feedback: `stars` 0 (none) to 5, `text` may be empty when stars are
/// given, `contact` how to reach the user ("" = not given). Nothing else about
/// the PC goes with it: the Glint version and the interface language only.
pub fn send_feedback(core: &Core, stars: u8, text: &str, contact: &str) -> Result<(), String> {
    let text = text.trim();
    let contact = cut(contact.trim(), MAX_CONTACT);
    let n = text.chars().count();
    if stars > MAX_STARS {
        return Err("Pick from one to five stars.".into());
    }
    if stars == 0 && n == 0 {
        return Err("Give some stars or write a few words before sending.".into());
    }
    if n > MAX_TEXT {
        return Err(format!("The text is too long: {n} characters, at most {MAX_TEXT} fit."));
    }
    if LAST_FEEDBACK.lock().unwrap().is_some_and(|t| t.elapsed() < PAUSE) {
        return Err("Wait a moment before sending more feedback.".into());
    }
    let url = endpoint();
    let language = core.settings().language.clone();
    let body = serde_json::to_string(&feedback_payload(stars, text, &contact, &language)).map_err(|e| format!("Could not prepare the feedback: {e}"))?;
    let result = post(&url, "fb", &body, None, "feedback");
    match &result {
        Ok(()) => {
            *LAST_FEEDBACK.lock().unwrap() = Some(Instant::now());
            util::log(&format!("feedback: sent ({stars} stars, {n} characters)"));
        }
        Err(e) => util::log(&format!("feedback: not sent: {e}")),
    }
    result
}

/// One request the test listener received.
#[derive(Clone, Default)]
struct Received {
    method: String,
    /// The request target, e.g. "/report?kind=bug".
    path: String,
    body: String,
}

/// A tiny HTTP server on 127.0.0.1 that keeps every request and answers with `status`.
struct Listener {
    url: String,
    got: std::sync::Arc<Mutex<Vec<Received>>>,
    status: std::sync::Arc<std::sync::atomic::AtomicU16>,
}

fn listen() -> std::io::Result<Listener> {
    use std::io::{Read, Write};
    let server = std::net::TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}/report", server.local_addr()?.port());
    let got = std::sync::Arc::new(Mutex::new(Vec::new()));
    let status = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(204));
    let (g, s) = (got.clone(), status.clone());
    std::thread::spawn(move || {
        for stream in server.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 16384];
            let mut head_end = None;
            let mut length = 0usize;
            let mut continued = false;
            loop {
                if let Some(h) = head_end {
                    if buf.len() >= h + length {
                        break;
                    }
                }
                let n = match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                if head_end.is_none() {
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(p + 4);
                        let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                        length = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
                        if head.contains("expect: 100-continue") && !continued {
                            continued = true;
                            let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
                        }
                    }
                }
            }
            let h = head_end.unwrap_or(buf.len());
            let head = String::from_utf8_lossy(&buf[..h]).to_string();
            let mut words = head.split_whitespace();
            let method = words.next().unwrap_or("").to_string();
            let path = words.next().unwrap_or("").to_string();
            g.lock().unwrap().push(Received { method, path, body: String::from_utf8_lossy(&buf[h..]).into_owned() });
            let answer = match s.load(Ordering::Relaxed) {
                400 => {
                    let body = r#"{"message": "Invalid Form Body", "code": 50035}"#;
                    format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                }
                429 => {
                    let body = r#"{"error": "Wait a moment before sending another report."}"#;
                    format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                }
                _ => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string(),
            };
            let _ = stream.write_all(answer.as_bytes());
            let _ = stream.flush();
        }
    });
    Ok(Listener { url, got, status })
}

/// The part of a multipart body named `name` (its headers and content).
fn part<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let at = body.find(&format!("name=\"{name}\""))?;
    let rest = &body[at..];
    let end = rest.find("\r\n--").unwrap_or(rest.len());
    Some(&rest[..end])
}

/// `glint.exe --bug-test <report.txt>`: headless checks against a listener on
/// 127.0.0.1. Never sends anything to the real server.
pub fn self_test(report_path: &str) {
    // First of all: no post of this process may leave 127.0.0.1 (see `post`).
    TEST_ONLY.store(true, Ordering::SeqCst);
    let lines: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let server = match listen() {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::write(report_path, format!("FAIL could not start the test listener: {e}\n"));
            return;
        }
    };
    std::env::set_var("GLINT_REPORT_URL", &server.url);
    // Never go on unless every report of this test lands on the listener.
    if !endpoint().starts_with("http://127.0.0.1:") {
        let _ = fs::write(report_path, "FAIL the report address does not point at the test listener\n");
        return;
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_default();
    let has_profile = |t: &str| {
        let t = t.to_lowercase().replace('/', "\\").replace("\\\\", "\\");
        !profile.is_empty() && t.contains(&profile.to_lowercase())
    };
    // Lines in the log that name the profile folder in several spellings.
    let _ = fs::remove_file(util::data_dir().join("glint.log"));
    util::log(&format!("test: clips in {profile}\\Videos\\Glint"));
    util::log(&format!("test: upper case {}\\VIDEOS", profile.to_uppercase()));
    util::log(&format!("test: slashes {}/AppData/Roaming", profile.replace('\\', "/")));
    util::log(&format!("test: json {}", serde_json::to_string(&format!("{profile}\\x")).unwrap_or_default()));
    let core = Core::new();
    let count = || server.got.lock().unwrap().len();
    let last = || server.got.lock().unwrap().last().cloned().unwrap_or_default();
    let note = |line: String| lines.borrow_mut().push(line);
    let check = |name: &str, ok: bool| note(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));

    check("sanitize: both slash styles and any case", sanitize_with(r"a C:\Users\Bob\x c:/users/bob/y C:\\Users\\BOB\\z", r"C:\Users\bob") == r"a ~\x ~/y ~\\z");
    check("sanitize: a longer user name is not cut", sanitize_with(r"C:\Users\bobby\x", r"C:\Users\bob") == r"C:\Users\bobby\x");
    check("sanitize: non-English user name", sanitize_with(r"D:\Пользователи\Ärger\a C:\Users\김민수\clips", r"C:\Users\김민수") == r"D:\Пользователи\Ärger\a ~\clips");

    let info = info(&core);
    note(format!("INFO configured={} version={} system lines={} log lines={}", info.configured, info.version, info.system.lines().count(), info.log.lines().count()));
    check("info: configured", info.configured);
    check("info: version and Windows build", info.system.contains(&format!("Glint {}", env!("CARGO_PKG_VERSION"))) && info.system.contains("(build "));
    check("info: graphics, CPU, capture, buffer and language lines", ["Graphics: ", "CPU: ", "Capture: ", "Replay buffer: ", "Interface language: "].iter().all(|k| info.system.contains(k)));
    check("info: log holds the test lines", info.log.contains("test: clips in ~\\Videos") && info.log.contains("test: slashes ~/AppData"));
    check("info: no profile path in system or log", !has_profile(&info.system) && !has_profile(&info.log));

    // Refused before any request is made.
    let text = "The clip I saved had no sound from the game.";
    check("empty text is refused", send(&core, "   ", "", false).is_err_and(|e| e.contains("Describe")));
    check("very short text is refused", send(&core, "  broken  ", "", false).is_err_and(|e| e.contains("few more words")));
    check("too long text is refused", send(&core, &"x".repeat(MAX_TEXT + 1), "", false).is_err_and(|e| e.contains("too long")));
    check("no request was made for refused text", count() == 0);

    // A report without the log.
    let r = send(&core, text, "", false);
    check(&format!("report is sent: {r:?}"), r.is_ok() && count() == 1);
    let req = last();
    let json = part(&req.body, "payload_json").and_then(|p| p.split_once("\r\n\r\n")).and_then(|(_, j)| serde_json::from_str::<serde_json::Value>(j).ok()).unwrap_or_default();
    check("it is a POST to /report?kind=bug", req.method == "POST" && req.path == "/report?kind=bug");
    check("payload_json is JSON", json.is_object());
    let content = json["content"].as_str().unwrap_or("");
    check("content names the version and pings no one (the server adds the pings)", content == format!("Bug report · Glint {}", env!("CARGO_PKG_VERSION")));
    check("allowed_mentions: none", json["allowed_mentions"] == serde_json::json!({ "parse": [] }));
    check("embed carries the text", json["embeds"][0]["description"].as_str() == Some(text));
    let fields = json["embeds"][0]["fields"].as_array().cloned().unwrap_or_default();
    check("no Contact field when none was given", !fields.iter().any(|f| f["name"] == "Contact"));
    let system = fields.iter().find(|f| f["name"] == "System").and_then(|f| f["value"].as_str()).unwrap_or("");
    check("System field in a code block, within 1024", system.starts_with("```") && system.ends_with("```") && system.chars().count() <= 1024 && system.contains("Glint "));
    check("no log part when not asked", part(&req.body, "files[0]").is_none());
    check("no profile path in the request", !has_profile(&req.body));

    // The 30-second pause.
    check("a second report at once is refused", send(&core, text, "", false).is_err_and(|e| e.contains("Wait a moment")) && count() == 1);
    *LAST_SENT.lock().unwrap() = None;

    // With the log and a contact.
    let r = send(&core, text, "  someone#1234  ", true);
    check(&format!("report with the log is sent: {r:?}"), r.is_ok() && count() == 2);
    let req = last();
    let log = part(&req.body, "files[0]").unwrap_or("");
    check("log part is present, named glint-log.txt", log.contains("filename=\"glint-log.txt\""));
    check("log part holds the log, with ~ for the profile", log.contains("test: clips in ~\\Videos\\Glint") && log.contains("test: upper case ~\\VIDEOS"));
    check("no profile path in the log part", !has_profile(log) && !has_profile(&req.body));
    check("Contact field is present", req.body.contains("\"name\":\"Contact\"") && req.body.contains("someone#1234"));
    let leftovers = fs::read_dir(util::tmp_dir()).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("bug-")).count();
    check("temporary files were removed", leftovers == 0);

    // Discord says no.
    *LAST_SENT.lock().unwrap() = None;
    server.status.store(400, Ordering::Relaxed);
    let r = send(&core, text, "", false);
    note(format!("INFO HTTP 400 gives: {r:?}"));
    check("HTTP 400 is an error with its code", r.as_ref().is_err_and(|e| e.contains("400")) && count() == 3);
    check("the error does not show the address", r.as_ref().is_err_and(|e| !e.contains(&server.url) && !e.contains("127.0.0.1") && !e.contains("/report")));
    check("a failed report does not start the pause", send(&core, text, "", false).is_err_and(|e| e.contains("400")) && count() == 4);
    // The server's own refusal (rate limit, not set up) is shown in its words.
    server.status.store(429, Ordering::Relaxed);
    let r = send(&core, text, "", false);
    note(format!("INFO server 429 gives: {r:?}"));
    check("the server's error text is shown as it is", r.as_ref().is_err_and(|e| e == "Wait a moment before sending another report."));
    server.status.store(204, Ordering::Relaxed);

    // Nothing listening: a readable error, still without the address.
    *LAST_SENT.lock().unwrap() = None;
    let dead = std::net::TcpListener::bind("127.0.0.1:0").ok().and_then(|l| l.local_addr().ok()).map(|a| format!("http://127.0.0.1:{}/report", a.port())).unwrap_or_default();
    std::env::set_var("GLINT_REPORT_URL", &dead);
    let r = send(&core, text, "", false);
    note(format!("INFO no listener gives: {r:?}"));
    check("no answer is an error without the address", r.as_ref().is_err_and(|e| e.contains("Could not reach Discord") && !e.contains(&dead)));
    std::env::set_var("GLINT_REPORT_URL", &server.url);

    // The test guard: an address that is not the listener is refused before curl runs.
    *LAST_SENT.lock().unwrap() = None;
    let before = count();
    std::env::set_var("GLINT_REPORT_URL", "https://example.invalid/report");
    let r = send(&core, text, "", false);
    let rf = send_feedback(&core, 4, "guard", "");
    std::env::set_var("GLINT_REPORT_URL", &server.url);
    check("test guard: a post to any other address is refused", r.is_err_and(|e| e.starts_with("Test run")) && rf.is_err_and(|e| e.starts_with("Test run")) && count() == before);
    *LAST_SENT.lock().unwrap() = None;
    *LAST_FEEDBACK.lock().unwrap() = None;

    // ---- Feedback (the heart button).
    let json_of = |req: &Received| part(&req.body, "payload_json").and_then(|p| p.split_once("\r\n\r\n")).and_then(|(_, j)| serde_json::from_str::<serde_json::Value>(j).ok()).unwrap_or_default();
    let field = |json: &serde_json::Value, name: &str| json["embeds"][0]["fields"].as_array().and_then(|a| a.iter().find(|f| f["name"] == name)).and_then(|f| f["value"].as_str().map(String::from));
    let no_ping = |json: &serde_json::Value, body: &str| json["allowed_mentions"] == serde_json::json!({ "parse": [] }) && !json["content"].as_str().unwrap_or("@").contains('@') && !body.contains("<@");
    let version = env!("CARGO_PKG_VERSION");
    let before = count();
    check("feedback: nothing at all is refused", send_feedback(&core, 0, "   ", "someone").is_err_and(|e| e.contains("Give some stars")));
    check("feedback: six stars are refused", send_feedback(&core, 6, "nice", "").is_err_and(|e| e.contains("one to five")));
    check("feedback: too long text is refused", send_feedback(&core, 3, &"x".repeat(MAX_TEXT + 1), "").is_err_and(|e| e.contains("too long")));
    check("feedback: no request was made for refused feedback", count() == before);

    let fb_text = "The editor is great, but the timeline could zoom further.";
    let r = send_feedback(&core, 4, &format!("  {fb_text}  "), "  someone#1234  ");
    check(&format!("feedback is sent: {r:?}"), r.is_ok() && count() == before + 1);
    let req = last();
    let json = json_of(&req);
    note(format!("INFO feedback payload: {json}"));
    check("feedback: a POST to /report?kind=feedback with payload_json", req.method == "POST" && req.path == "/report?kind=feedback" && json.is_object());
    check("feedback: content says Feedback and the version, not Bug report", json["content"] == format!("Feedback · Glint {version}"));
    check("feedback: embed title is Feedback", json["embeds"][0]["title"] == "Feedback");
    check("feedback: pings no one (allowed_mentions parse [], no mention)", no_ping(&json, &req.body));
    check("feedback: stars as characters and as 4 / 5", field(&json, "Stars").as_deref() == Some("★★★★☆ 4 / 5"));
    check("feedback: the text, trimmed", json["embeds"][0]["description"].as_str() == Some(fb_text));
    check("feedback: the contact, trimmed", field(&json, "Contact").as_deref() == Some("someone#1234"));
    check("feedback: Glint version and language", field(&json, "Glint") == Some(format!("{version} · language {}", core.settings().language)));
    check("feedback: no system lines, no log", field(&json, "System").is_none() && part(&req.body, "files[0]").is_none() && !req.body.contains("test: clips") && !req.body.contains("Graphics:") && !has_profile(&req.body));
    check("feedback: exactly the three fields", json["embeds"][0]["fields"].as_array().map(|a| a.len()) == Some(3));

    check("feedback: a second one at once is refused", send_feedback(&core, 5, "again", "").is_err_and(|e| e.contains("Wait a moment")) && count() == before + 1);
    *LAST_SENT.lock().unwrap() = None;
    check("feedback: its pause does not hold back a bug report", send(&core, text, "", false).is_ok() && count() == before + 2);
    *LAST_FEEDBACK.lock().unwrap() = None;

    let r = send_feedback(&core, 5, "", "");
    let req = last();
    let json = json_of(&req);
    check(&format!("feedback: stars alone are sent: {r:?}"), r.is_ok() && count() == before + 3);
    check("feedback: stars alone have no description and no Contact", json["embeds"][0].get("description").is_none() && field(&json, "Contact").is_none() && field(&json, "Stars").as_deref() == Some("★★★★★ 5 / 5"));
    *LAST_FEEDBACK.lock().unwrap() = None;

    let r = send_feedback(&core, 0, "@everyone <@123456789> @here love it", "");
    let req = last();
    let json = json_of(&req);
    check(&format!("feedback: text alone is sent: {r:?}"), r.is_ok() && count() == before + 4);
    check("feedback: text alone says no stars given", field(&json, "Stars").as_deref() == Some("No stars given"));
    check("feedback: mentions typed in the text ping no one", json["allowed_mentions"] == serde_json::json!({ "parse": [] }) && json["content"] == format!("Feedback · Glint {version}"));
    *LAST_FEEDBACK.lock().unwrap() = None;

    server.status.store(400, Ordering::Relaxed);
    let r = send_feedback(&core, 2, "", "");
    note(format!("INFO feedback HTTP 400 gives: {r:?}"));
    check("feedback: HTTP 400 is an error naming the feedback, without the address", r.as_ref().is_err_and(|e| e.contains("400") && e.contains("feedback") && !e.contains("127.0.0.1") && !e.contains("/report")) && count() == before + 5);
    check("feedback: a failed one does not start the pause", send_feedback(&core, 2, "", "").is_err_and(|e| e.contains("400")) && count() == before + 6);
    server.status.store(204, Ordering::Relaxed);
    let leftovers = fs::read_dir(util::tmp_dir()).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("fb-")).count();
    check("feedback: temporary files were removed", leftovers == 0);
    let fi = feedback_info(&core);
    check("feedback info: configured, version, language", fi.configured && fi.version == version && fi.language == core.settings().language);

    let _ = fs::write(report_path, lines.borrow().join("\n") + "\n");
}
