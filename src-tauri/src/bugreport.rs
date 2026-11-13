//! "Report a bug": a message from the user, posted to the developer's Discord.
//!
//! The report goes to a Discord webhook built into the program (see build.rs:
//! it comes from the git-ignored `bug-webhook.txt` and is stored lightly
//! obfuscated) and pings the developers named there.
//!
//! What is sent, and only when the user presses Send: their text, the contact
//! they typed (if any), the `system` lines they were shown (Glint and Windows
//! versions, graphics card, CPU, capture settings) and, if they tick the box,
//! the last lines of `glint.log`. Every occurrence of the user's profile folder
//! is replaced by `~` first, so the Windows user name does not leave the PC.
//!
//! Network use: one HTTPS POST to discord.com per report, with curl.exe (it
//! ships with Windows 10 and 11). `GLINT_BUG_HOOK_URL` replaces the address at
//! run time; the self-test points it at a listener on 127.0.0.1.
use crate::core::Core;
use crate::{util, win};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Must match `KEY` in build.rs.
const KEY: &[u8] = b"glint:bug-report:v1/k9#Qz";
/// The obfuscated webhook URL and ping ids ("" when the build had no bug-webhook.txt).
const HOOK: &str = env!("GLINT_BUG_HOOK");
const PINGS: &str = env!("GLINT_BUG_PINGS");

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

/// Undo build.rs's `hide`.
fn reveal(hex: &str) -> String {
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok())
        .enumerate()
        .map(|(i, b)| b ^ KEY[i % KEY.len()] ^ (i as u8).wrapping_mul(31))
        .collect();
    String::from_utf8(bytes).unwrap_or_default()
}

/// Where reports go and whom they ping.
struct Hook {
    url: String,
    ids: Vec<String>,
}

fn hook() -> Hook {
    let url = match std::env::var("GLINT_BUG_HOOK_URL") {
        Ok(u) if !u.trim().is_empty() => u.trim().to_string(),
        _ => reveal(HOOK),
    };
    let ids = reveal(PINGS).split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    Hook { url, ids }
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
    BugInfo { configured: !hook().url.is_empty(), version: env!("CARGO_PKG_VERSION").to_string(), system: system_text(core), log: log_text() }
}

/// At most `max` characters, with "…" at the end when something was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

/// The `payload_json` part of the post.
fn payload(ids: &[String], text: &str, contact: &str, system: &str) -> serde_json::Value {
    let pings: Vec<String> = ids.iter().map(|id| format!("<@{id}>")).collect();
    let title = format!("Bug report · Glint {}", env!("CARGO_PKG_VERSION"));
    let content = if pings.is_empty() { title } else { format!("{}\n{title}", pings.join(" ")) };
    let mut fields = Vec::new();
    if !contact.is_empty() {
        fields.push(serde_json::json!({ "name": "Contact", "value": cut(contact, 1024), "inline": false }));
    }
    // A field holds 1024 characters; the code fence takes 8 of them.
    let system = system.replace("```", "'''");
    fields.push(serde_json::json!({ "name": "System", "value": format!("```\n{}\n```", cut(&system, 1024 - 8)), "inline": false }));
    serde_json::json!({
        "content": cut(&content, 2000),
        "allowed_mentions": { "parse": [], "users": ids },
        "embeds": [{ "description": cut(text, 4096), "color": 0xf5b83d, "fields": fields }],
    })
}

/// A short reason, made safe to show: never the webhook address, never the profile folder.
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
    let hook = hook();
    if hook.url.is_empty() {
        return Err("Bug reports are not set up in this copy of Glint.".into());
    }
    let system = system_text(core);
    let body = serde_json::to_string(&payload(&hook.ids, text, &contact, &system)).map_err(|e| format!("Could not prepare the report: {e}"))?;

    // curl reads both parts from files named relative to its working folder,
    // so a profile path with spaces or non-English letters never has to pass
    // through its command line.
    let dir = util::tmp_dir();
    let id = util::new_id();
    let (json_name, log_name) = (format!("bug-{id}.json"), format!("bug-{id}-log.txt"));
    let result = (|| {
        fs::write(dir.join(&json_name), &body).map_err(|e| format!("Could not prepare the report: {e}"))?;
        let mut cmd = util::hidden_cmd(Path::new("curl.exe"));
        cmd.current_dir(&dir)
            .args(["-sS", "--max-time", "20", "-A", "Glint", "-H", "Expect:", "-o", "-", "-w", "\n%{http_code}"])
            .arg("-F")
            .arg(format!("payload_json=<{json_name};type=application/json"));
        if include_log {
            let log = log_text();
            fs::write(dir.join(&log_name), if log.is_empty() { "(the log is empty)".to_string() } else { log }).map_err(|e| format!("Could not attach the log: {e}"))?;
            cmd.arg("-F").arg(format!("files[0]=@{log_name};filename=glint-log.txt;type=text/plain"));
        }
        let out = cmd.arg(&hook.url).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not run curl.exe: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let (answer, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_str()));
        let code: u16 = code.trim().parse().unwrap_or(0);
        if code == 0 {
            let why = util::last_lines(&out.stderr, 1);
            return Err(scrub(&format!("Could not reach Discord. Check the internet connection and try again. ({})", if why.is_empty() { "no answer" } else { &why }), &hook.url));
        }
        if !(200..=299).contains(&code) {
            let detail = serde_json::from_str::<serde_json::Value>(answer.trim()).ok().and_then(|v| v["message"].as_str().map(|m| cut(m, 120))).unwrap_or_default();
            let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
            let msg = if code == 429 { format!("Discord is busy right now (HTTP 429{detail}). Try again in a minute.") } else { format!("Discord did not accept the report (HTTP {code}{detail}).") };
            return Err(scrub(&msg, &hook.url));
        }
        Ok(())
    })();
    let _ = fs::remove_file(dir.join(&json_name));
    let _ = fs::remove_file(dir.join(&log_name));
    match &result {
        Ok(()) => {
            *LAST_SENT.lock().unwrap() = Some(Instant::now());
            util::log(&format!("bug report: sent ({n} characters, log {})", if include_log { "attached" } else { "not attached" }));
        }
        Err(e) => util::log(&format!("bug report: not sent: {e}")),
    }
    result
}

/// One request the test listener received.
#[derive(Clone, Default)]
struct Received {
    method: String,
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
    use std::sync::atomic::Ordering;
    let server = std::net::TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}/api/webhooks/test/token", server.local_addr()?.port());
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
            let method = String::from_utf8_lossy(&buf[..h]).split_whitespace().next().unwrap_or("").to_string();
            g.lock().unwrap().push(Received { method, body: String::from_utf8_lossy(&buf[h..]).into_owned() });
            let answer = match s.load(Ordering::Relaxed) {
                400 => {
                    let body = r#"{"message": "Invalid Form Body", "code": 50035}"#;
                    format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
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
/// 127.0.0.1. Never sends anything to the real webhook.
pub fn self_test(report_path: &str) {
    use std::sync::atomic::Ordering;
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
    std::env::set_var("GLINT_BUG_HOOK_URL", &server.url);
    // Never go on unless every report of this test lands on the listener.
    if !hook().url.starts_with("http://127.0.0.1:") {
        let _ = fs::write(report_path, "FAIL the report address does not point at the test listener\n");
        return;
    }
    let built_in = reveal(HOOK);
    let ids: Vec<String> = hook().ids;
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
    check("it is a POST", req.method == "POST");
    check("payload_json is JSON", json.is_object());
    note(format!("INFO ping ids built in: {}", ids.len()));
    let content = json["content"].as_str().unwrap_or("");
    check("content pings every id", !ids.is_empty() && ids.iter().all(|id| content.contains(&format!("<@{id}>"))));
    check("content names the version", content.contains(&format!("Glint {}", env!("CARGO_PKG_VERSION"))));
    let users: Vec<String> = json["allowed_mentions"]["users"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    check("allowed_mentions: no parse, exactly the ids", json["allowed_mentions"]["parse"].as_array().is_some_and(|a| a.is_empty()) && users == ids);
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
    check("the error does not show the address", r.as_ref().is_err_and(|e| !e.contains(&server.url) && !e.contains("127.0.0.1") && !e.contains("webhooks")));
    check("a failed report does not start the pause", send(&core, text, "", false).is_err_and(|e| e.contains("400")) && count() == 4);
    server.status.store(204, Ordering::Relaxed);

    // Nothing listening: a readable error, still without the address.
    *LAST_SENT.lock().unwrap() = None;
    let dead = std::net::TcpListener::bind("127.0.0.1:0").ok().and_then(|l| l.local_addr().ok()).map(|a| format!("http://127.0.0.1:{}/api/webhooks/x/y", a.port())).unwrap_or_default();
    std::env::set_var("GLINT_BUG_HOOK_URL", &dead);
    let r = send(&core, text, "", false);
    note(format!("INFO no listener gives: {r:?}"));
    check("no answer is an error without the address", r.as_ref().is_err_and(|e| e.contains("Could not reach Discord") && !e.contains(&dead)));
    std::env::set_var("GLINT_BUG_HOOK_URL", &server.url);

    // The built-in destination: present, decodes to a Discord webhook, not stored as plain text.
    if built_in.is_empty() {
        note("INFO no bug-webhook.txt at build time: reports are not configured in this build".into());
    } else {
        check("built-in address decodes to a Discord webhook", built_in.starts_with("https://discord.com/api/webhooks/"));
        let exe = std::env::current_exe().ok().and_then(|p| fs::read(p).ok()).unwrap_or_default();
        let plain = built_in.as_bytes();
        check("the address is not in the exe as plain text", !exe.is_empty() && !exe.windows(plain.len()).any(|w| w == plain));
    }
    let _ = fs::write(report_path, lines.borrow().join("\n") + "\n");
}
