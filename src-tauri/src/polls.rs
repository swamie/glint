//! Polls from the developer (server/polls in the repository).
//!
//! The developer runs a small server and asks Glint's users questions. Where
//! that server is, is one line in the repository (`ADDRESS_URL`, read at most
//! once a day); empty, missing or unreachable means no polls and no error.
//! Only an `https://` address is used.
//!
//! What is sent: to list polls, a random number made for this install (16
//! random bytes, in its own file in the data folder: not from the hardware,
//! never in a bug report); a vote adds the answers and the optional comment.
//! Nothing at all while the setting `polls` is off.
//!
//! HTTP is done by curl.exe as in update.rs: no shell, a time limit, the JSON
//! body on stdin (`--data-binary @-`), the address after `--url`, so nothing a
//! voter typed can reach curl's command line.
use crate::core::Core;
use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The file that holds the server's address (one line, or empty for "no polls").
pub const ADDRESS_URL: &str = "https://raw.githubusercontent.com/swamie/glint/main/server/polls/address.txt";
/// How long the address is kept before it is read again.
const ADDRESS_FOR: u64 = 24 * 3600;
/// After a failed read of the address, try again after this long.
const ADDRESS_RETRY: u64 = 3600;
/// The first look after Glint starts, and the time between looks.
const FIRST_LOOK: u64 = 30;
const LOOK_EVERY: u64 = 6 * 3600;
/// A list asked for by hand (the top bar's button) refreshes at most this often.
const MANUAL_EVERY: u64 = 60;
pub const MAX_COMMENT: usize = 300;
const MAX_QUESTION: usize = 200;
const MAX_OPTION: usize = 80;
const MAX_POLLS: usize = 20;
const MAX_ANSWER: usize = 256 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct PollOption {
    pub id: i64,
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Results {
    pub total: i64,
    /// Option id (as text) -> votes.
    pub counts: BTreeMap<String, i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Poll {
    pub id: i64,
    pub question: String,
    pub options: Vec<PollOption>,
    pub multiple: bool,
    pub allow_comment: bool,
    pub opens: i64,
    /// Unix seconds when it ends (or ended); None = no end time.
    pub closes: Option<i64>,
    pub closed: bool,
    /// The options this install voted for; None = not voted.
    pub voted: Option<Vec<i64>>,
    /// Only once this install has voted, or the poll has ended.
    pub results: Option<Results>,
}

/// Where requests go. `Net::real()` in Glint; the self-test points it at its own listener.
#[derive(Clone, Debug)]
pub struct Net {
    pub address_url: String,
    /// Plain http to 127.0.0.1 is allowed (the self-test only).
    pub test: bool,
    pub max_time: u64,
}

impl Net {
    pub fn real() -> Net {
        Net { address_url: ADDRESS_URL.into(), test: false, max_time: 15 }
    }
}

/* ---------- the install number ---------- */

fn id_file() -> PathBuf {
    util::data_dir().join("polls-id.txt")
}

fn well_formed(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptGenRandom(alg: *mut std::ffi::c_void, buf: *mut u8, len: u32, flags: u32) -> i32;
}

/// 16 bytes from Windows' system random generator.
fn random16() -> Option<[u8; 16]> {
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
    let mut b = [0u8; 16];
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), b.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    (status == 0 && b.iter().any(|&x| x != 0)).then_some(b)
}

static ID_LOCK: Mutex<()> = Mutex::new(());

/// This install's random number (32 hex digits), made on first use and kept in its own file.
pub fn install_id() -> Option<String> {
    let _guard = ID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = id_file();
    if let Ok(text) = fs::read_to_string(&path) {
        let id = text.trim().to_string();
        if well_formed(&id) {
            return Some(id);
        }
    }
    let id: String = random16()?.iter().map(|b| format!("{b:02x}")).collect();
    fs::write(&path, &id).ok()?;
    Some(id)
}

/* ---------- addresses ---------- */

/// The server's address if Glint may use it: `https://host[:port][/path]` with nothing odd in
/// it (no spaces, user names, queries or fragments); `http://127.0.0.1:port` only when `test`.
/// Returned without a trailing slash.
pub fn acceptable(addr: &str, test: bool) -> Option<String> {
    let a = addr.trim().trim_end_matches('/');
    if a.len() > 200 {
        return None;
    }
    let rest = if let Some(r) = a.strip_prefix("https://") {
        r
    } else if test && a.starts_with("http://127.0.0.1:") {
        &a["http://".len()..]
    } else {
        return None;
    };
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    let (name, port) = match host.rsplit_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    let name_ok = !name.is_empty() && name.len() <= 253 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') && !name.starts_with(['-', '.']) && !name.ends_with('-');
    let port_ok = port.map_or(true, |p|!p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n)));
    let path_ok = path.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~/".contains(&b));
    // Only a public name: no IP literal (a real top-level domain has a letter and is no "0x.."
    // number, so 1.2.3.4, 2130706433 and 0x7f.0.0.1 are out), no single-label host, nothing
    // that names this PC or the local network. (The self-test's http://127.0.0.1 is let in above.)
    let lower = name.to_ascii_lowercase();
    let tld = lower.rsplit('.').next().unwrap_or("");
    let local = test && a.starts_with("http://127.0.0.1:");
    let public = local
        || (lower.contains('.')
            && tld.bytes().any(|b| b.is_ascii_alphabetic())
            && !tld.starts_with("0x")
            && !["localhost", "local", "lan", "internal", "home", "intranet"].contains(&tld));
    (name_ok && port_ok && path_ok && public).then(|| a.to_string())
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct AddressCache {
    source: String,
    address: String,
    fetched: u64,
    /// The read failed: try again after `ADDRESS_RETRY` instead of a day.
    failed: bool,
}

fn cache_file() -> PathBuf {
    util::data_dir().join("polls-address.json")
}

fn now() -> u64 {
    util::unix_ms() / 1000
}

/// The server's address ("" = none), from the cache when it is fresh, else read again.
fn address(net: &Net) -> Result<String, String> {
    let cache: AddressCache = fs::read_to_string(cache_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let keep = if cache.failed { ADDRESS_RETRY } else { ADDRESS_FOR };
    if cache.source == net.address_url && now() < cache.fetched + keep && now() + 60 >= cache.fetched {
        return Ok(cache.address);
    }
    let save = |address: &str, failed: bool| {
        let c = AddressCache { source: net.address_url.clone(), address: address.into(), fetched: now(), failed };
        if let Ok(t) = serde_json::to_string(&c) {
            let _ = fs::write(cache_file(), t);
        }
    };
    let (code, body) = match http(net, &net.address_url, None) {
        Ok(r) => r,
        Err(e) => {
            save("", true);
            return Err(e);
        }
    };
    if code == 404 {
        save("", false); // no address file (yet): no polls
        return Ok(String::new());
    }
    if code != 200 {
        save("", true);
        return Err(format!("the address file answered HTTP {code}"));
    }
    let line = String::from_utf8_lossy(&body).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    if line.is_empty() {
        save("", false);
        return Ok(String::new());
    }
    match acceptable(&line, net.test) {
        Some(a) => {
            save(&a, false);
            Ok(a)
        }
        None => {
            save("", false);
            Err("the server address is not an https:// address; it was not used".into())
        }
    }
}

/// Forget the cached address (the self-test changes it between steps).
pub fn forget_address() {
    let _ = fs::remove_file(cache_file());
}

/* ---------- HTTP ---------- */

/// Set by the self-test: from then on only 127.0.0.1 may be asked.
static TEST_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One request with curl.exe: GET, or POST with `body` (JSON) on curl's stdin. (HTTP status, answer).
fn http(net: &Net, url: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>), String> {
    let local = url.starts_with("http://127.0.0.1:");
    if TEST_ONLY.load(std::sync::atomic::Ordering::SeqCst) && !local {
        return Err("test run: only 127.0.0.1 may be asked".into());
    }
    if !(url.starts_with("https://") || (net.test && local)) {
        return Err("not an https:// address".into());
    }
    let mut cmd = util::hidden_cmd(Path::new("curl.exe"));
    cmd.args(["-sS", "--globoff", "--proto", if net.test { "=http,https" } else { "=https" }, "--max-time"])
        .arg(net.max_time.to_string())
        .args(["--connect-timeout", "10", "--max-filesize", "1048576", "-A", "Glint", "-H", "Accept: application/json", "-o", "-", "-w", "\n%{http_code}"]);
    if body.is_some() {
        cmd.args(["-H", "Content-Type: application/json", "-H", "Expect:", "--data-binary", "@-"]);
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    cmd.arg("--url").arg(url).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run curl.exe: {e}"))?;
    if let Some(b) = body {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b);
        } // dropped here: curl sees the end of the body
    }
    // stderr is read on the side so a chatty curl can never block on a full pipe.
    let err_reader = child.stderr.take().map(|mut e| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut e, &mut buf);
            buf
        })
    });
    // The answer is read as it arrives; past MAX_ANSWER curl is stopped (whatever
    // --max-filesize did: a server may send no Content-Length).
    let mut stdout = Vec::new();
    let mut too_large = false;
    if let Some(mut out) = child.stdout.take() {
        let mut chunk = [0u8; 16384];
        loop {
            match std::io::Read::read(&mut out, &mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    stdout.extend_from_slice(&chunk[..n]);
                    if stdout.len() > MAX_ANSWER + 16 {
                        too_large = true;
                        let _ = child.kill();
                        break;
                    }
                }
            }
        }
    }
    let status = child.wait().ok();
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    // curl 63 = its own size limit (a backstop at 1 MB).
    if too_large || status.and_then(|s| s.code()) == Some(63) {
        return Err("the answer was too large".into());
    }
    // curl failed part-way (a time-out mid-answer, a dropped connection): never read a cut-off answer.
    if !status.is_some_and(|s| s.success()) {
        let why = util::last_lines(&stderr, 1);
        return Err(format!("could not reach the server ({})", if why.is_empty() { "the answer was cut off".into() } else { why }));
    }
    let tail = stdout.iter().rposition(|&b| b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&stdout[tail..]).trim().parse().unwrap_or(0);
    stdout.truncate(tail);
    if code == 0 {
        let why = util::last_lines(&stderr, 1);
        return Err(format!("could not reach the server ({})", if why.is_empty() { "no answer".into() } else { why }));
    }
    if stdout.len() > MAX_ANSWER {
        return Err("the answer was too large".into());
    }
    Ok((code, stdout))
}

/// Keep only what Glint can show, at the server's own limits.
fn tidy(mut p: Poll) -> Option<Poll> {
    let cut = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>().trim().to_string();
    p.question = cut(&p.question, MAX_QUESTION);
    p.options.truncate(8);
    for o in &mut p.options {
        o.text = cut(&o.text, MAX_OPTION);
    }
    (p.id > 0 && !p.question.is_empty() && p.options.len() >= 2).then_some(p)
}

fn read_polls(body: &[u8]) -> Result<Vec<Poll>, String> {
    #[derive(Deserialize)]
    struct Answer {
        polls: Vec<Poll>,
    }
    let a: Answer = serde_json::from_slice(body).map_err(|_| "the server sent an answer Glint could not read".to_string())?;
    Ok(a.polls.into_iter().filter_map(tidy).take(MAX_POLLS).collect())
}

/// The open and recently closed polls. Nothing is asked while `enabled` is false.
pub fn list_with(net: &Net, enabled: bool) -> Result<Vec<Poll>, String> {
    if !enabled {
        return Ok(Vec::new());
    }
    let base = address(net)?;
    if base.is_empty() {
        return Ok(Vec::new());
    }
    let id = install_id().ok_or("could not make the install number")?;
    let (code, body) = http(net, &format!("{base}/v1/polls?install={id}"), None)?;
    if code != 200 {
        return Err(format!("the server answered HTTP {code}"));
    }
    read_polls(&body)
}

/// Vote on `poll`. The error is a plain English sentence for the UI.
pub fn vote_with(net: &Net, enabled: bool, poll: i64, options: &[i64], comment: &str) -> Result<Poll, String> {
    if !enabled {
        return Err("Polls are off in Settings.".into());
    }
    let comment = comment.trim();
    if options.is_empty() {
        return Err("Pick an answer first.".into());
    }
    if comment.chars().count() > MAX_COMMENT {
        return Err(format!("A comment can be at most {MAX_COMMENT} characters."));
    }
    let base = address(net).map_err(|e| {
        util::log(&format!("polls: {e}"));
        "Could not reach the polls server. Try again later.".to_string()
    })?;
    if base.is_empty() {
        return Err("Could not reach the polls server. Try again later.".into());
    }
    let id = install_id().ok_or("Could not reach the polls server. Try again later.")?;
    let mut body = serde_json::json!({ "poll": poll, "options": options, "install": id });
    if !comment.is_empty() {
        body["comment"] = comment.into();
    }
    let (code, answer) = http(net, &format!("{base}/v1/vote"), Some(body.to_string().as_bytes())).map_err(|e| {
        util::log(&format!("polls: vote not sent: {}", e.replace(&id, "…")));
        "Could not reach the polls server. Try again later.".to_string()
    })?;
    match code {
        200 => serde_json::from_slice::<Poll>(&answer).ok().and_then(tidy).ok_or_else(|| {
            util::log("polls: the answer to a vote could not be read");
            "The vote was not accepted.".to_string()
        }),
        409 => Err("This poll has ended.".into()),
        429 => Err("Too many votes from this network today. Try again later.".into()),
        _ => {
            util::log(&format!("polls: vote refused (HTTP {code})"));
            Err("The vote was not accepted.".into())
        }
    }
}

/* ---------- in Glint ---------- */

/// The last list, and when it was last asked for (unix seconds; 0 = never).
static CACHE: Mutex<(u64, Vec<Poll>)> = Mutex::new((0, Vec::new()));

/// Ask the server now (the setting must be on) and tell the window.
fn refresh(core: &Core) {
    CACHE.lock().unwrap().0 = now();
    match list_with(&Net::real(), core.settings().polls) {
        Ok(polls) => {
            CACHE.lock().unwrap().1 = polls.clone();
            core.emit("polls-changed", polls);
        }
        Err(e) => {
            let id = fs::read_to_string(id_file()).unwrap_or_default();
            let line = if id.trim().is_empty() { e } else { e.replace(id.trim(), "…") };
            util::log(&format!("polls: {line}"));
        }
    }
}

/// The background look: 30 s after start, then every 6 hours, never while the setting is off.
pub fn spawn(core: Arc<Core>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(FIRST_LOOK));
        loop {
            let last = CACHE.lock().unwrap().0;
            if core.settings().polls && now() >= last + LOOK_EVERY {
                refresh(&core);
            }
            std::thread::sleep(Duration::from_secs(60));
        }
    });
}

type St<'a> = tauri::State<'a, Arc<Core>>;

/// The polls known (no network), or with `refresh` asked again when the last look is over a
/// minute old. Empty while the setting is off.
#[tauri::command(async)]
pub fn polls_list(core: St, refresh: Option<bool>) -> Vec<Poll> {
    if !core.settings().polls {
        return Vec::new();
    }
    if refresh == Some(true) && now() >= CACHE.lock().unwrap().0 + MANUAL_EVERY {
        self::refresh(&core);
    }
    CACHE.lock().unwrap().1.clone()
}

/// Vote; the answer is the poll with its results.
#[tauri::command(async)]
pub fn polls_vote(core: St, poll: i64, options: Vec<i64>, comment: Option<String>) -> Result<Poll, String> {
    let p = vote_with(&Net::real(), core.settings().polls, poll, &options, comment.as_deref().unwrap_or(""))?;
    let mut cache = CACHE.lock().unwrap();
    match cache.1.iter_mut().find(|x| x.id == p.id) {
        Some(x) => *x = p.clone(),
        None => cache.1.push(p.clone()),
    }
    core.emit("polls-changed", cache.1.clone());
    Ok(p)
}

/* ---------- self-test ---------- */

/// What the test server does with the next requests.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Normal,
    Broken,
    Hang,
    /// 2 MB, slowly, with no Content-Length.
    Big,
}

struct TestServer {
    base: String,
    /// Every request: (method, path with query, body).
    got: Arc<Mutex<Vec<(String, String, String)>>>,
    mode: Arc<Mutex<Mode>>,
    /// What /address.txt answers.
    address: Arc<Mutex<String>>,
}

impl TestServer {
    fn count(&self) -> usize {
        self.got.lock().unwrap().len()
    }
    fn set(&self, m: Mode) {
        *self.mode.lock().unwrap() = m;
    }
}

fn poll_json(id: i64, voted: Option<Vec<i64>>) -> serde_json::Value {
    let results = voted.as_ref().map(|v| serde_json::json!({ "total": 3, "counts": { "11": 2 + i64::from(v.contains(&11)), "12": 1 } }));
    serde_json::json!({
        "id": id, "question": "Which mode next?", "options": [{ "id": 11, "text": "Ranked" }, { "id": 12, "text": "Casual" }],
        "multiple": false, "allow_comment": true, "opens": 1_700_000_000, "closes": 4_000_000_000i64, "closed": false,
        "voted": voted, "results": results,
    })
}

fn test_server() -> std::io::Result<TestServer> {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let base = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let got = Arc::new(Mutex::new(Vec::new()));
    let mode = Arc::new(Mutex::new(Mode::Normal));
    let address = Arc::new(Mutex::new(base.clone()));
    let (g, m, a) = (got.clone(), mode.clone(), address.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (g, m, a) = (g.clone(), m.clone(), a.clone());
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let mut head_end = None;
                let mut length = 0usize;
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
                        }
                    }
                }
                let h = head_end.unwrap_or(buf.len());
                let first = String::from_utf8_lossy(&buf[..h]).lines().next().unwrap_or("").to_string();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let body = String::from_utf8_lossy(&buf[h..]).into_owned();
                g.lock().unwrap().push((method.clone(), path.clone(), body.clone()));
                let mode = *m.lock().unwrap();
                let (status, answer) = if path == "/address.txt" {
                    (200, a.lock().unwrap().clone())
                } else if mode == Mode::Big {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n");
                    let block = vec![b' '; 64 * 1024];
                    for _ in 0..32 {
                        if stream.write_all(&block).is_err() {
                            break; // Glint stopped reading: the test wants exactly that
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    return;
                } else if mode == Mode::Hang {
                    std::thread::sleep(Duration::from_secs(20)); // never answers in time
                    return;
                } else if mode == Mode::Broken {
                    (200, "{\"polls\": [ this is not json".to_string())
                } else if path.starts_with("/v1/polls?install=") {
                    (200, serde_json::json!({ "polls": [poll_json(7, None), poll_json(8, Some(vec![12])), { "id": 9, "question": "", "options": [] }] }).to_string())
                } else if method == "POST" && path == "/v1/vote" {
                    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    if v["poll"] == 99 {
                        (409, "{\"error\": \"This poll has ended.\"}".to_string())
                    } else {
                        let opts: Vec<i64> = v["options"].as_array().map(|l| l.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
                        (200, poll_json(v["poll"].as_i64().unwrap_or(0), Some(opts)).to_string())
                    }
                } else {
                    (404, "{}".to_string())
                };
                let reason = if status == 200 { "OK" } else { "Error" };
                let _ = stream.write_all(format!("HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).as_bytes());
                let _ = stream.flush();
            });
        }
    });
    Ok(TestServer { base, got, mode, address })
}

/// `glint.exe --polls-test <report.txt>` (needs GLINT_DATA_DIR): against a tiny server this test
/// opens on 127.0.0.1. No other address is ever asked.
pub fn self_test(report_path: &str) {
    TEST_ONLY.store(true, std::sync::atomic::Ordering::SeqCst);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let mut lines: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool| lines.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));
    let server = match test_server() {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::write(report_path, format!("FAIL could not start the test server: {e}\n"));
            return;
        }
    };
    let net = Net { address_url: format!("{}/address.txt", server.base), test: true, max_time: 3 };

    // The install number.
    let a = install_id().unwrap_or_default();
    let b = install_id().unwrap_or_default();
    check("the install number is 32 lower-case hex digits", well_formed(&a));
    check("the install number is stable", !a.is_empty() && a == b && fs::read_to_string(id_file()).map(|t| t.trim() == a).unwrap_or(false));
    let _ = fs::remove_file(id_file());
    let c = install_id().unwrap_or_default();
    check("a new install gets a different number", well_formed(&c) && c != a);
    let id = c;
    check("the install number is not in the settings", !fs::read_to_string(util::data_dir().join("settings.json")).unwrap_or_default().contains(&id));

    // Which addresses are used.
    check("an https address is accepted", acceptable("https://polls.example.com", false).as_deref() == Some("https://polls.example.com") && acceptable(" https://polls.example.com:8443/glint/ \n", false).as_deref() == Some("https://polls.example.com:8443/glint"));
    let refused = ["http://polls.example.com", "ftp://polls.example.com", "polls.example.com", "https://", "https://a b.com", "https://user@host.com", "https://host.com/?x=1", "https://host.com/#x", "https://host.com:99999", "https://-x.com", "--output=x", "https://host.com/a b", "http://127.0.0.1:8787"];
    let local_hosts = ["https://127.0.0.1", "https://10.0.0.1:8443", "https://192.168.1.20/polls", "https://2130706433", "https://0x7f.0.0.1", "https://127.1", "https://localhost", "https://LOCALHOST:8443", "https://polls.localhost", "https://nas.local", "https://router.lan", "https://polls.corp.internal", "https://intranet"];
    check("an IP literal, localhost, .local, .lan or .internal is refused", local_hosts.iter().all(|r| acceptable(r, false).is_none() && acceptable(r, true).is_none()));
    check("ordinary public names are still accepted", ["https://polls.example.com", "https://glint-polls.example.co.uk:8443/v1", "https://xn--bcher-kva.example"].iter().all(|r| acceptable(r, false).is_some()));
    check("an address that is not https is refused", refused.iter().all(|r| acceptable(r, false).is_none()));
    check("plain http only to 127.0.0.1, only in the test", acceptable(&server.base, true).is_some() && acceptable("http://10.0.0.1:80", true).is_none() && acceptable("http://localhost:80", true).is_none());

    // The setting off: no request at all.
    let before = server.count();
    let off = list_with(&net, false);
    let off_vote = vote_with(&net, false, 7, &[11], "");
    check("the setting off: no polls and no request at all", off.as_ref().is_ok_and(|l| l.is_empty()) && off_vote.is_err() && server.count() == before);

    // The list.
    let list = list_with(&net, true);
    let got = server.got.lock().unwrap().clone();
    check(&format!("the list: two polls read, the empty one left out ({:?})", list.as_ref().map(|l| l.len())), list.as_ref().is_ok_and(|l| l.len() == 2 && l[0].id == 7 && l[0].results.is_none() && l[0].voted.is_none() && l[1].voted == Some(vec![12]) && l[0].options[0].text == "Ranked"));
    check("the list asked for the address, then for the polls with the install number", got.len() == before + 2 && got[before].1 == "/address.txt" && got[before + 1].1 == format!("/v1/polls?install={id}") && got[before + 1].0 == "GET");
    let before = server.count();
    let _ = list_with(&net, true);
    check("the address is kept (asked once a day)", server.count() == before + 1 && server.got.lock().unwrap().last().is_some_and(|r| r.1.starts_with("/v1/polls")));

    // A vote: the body goes on stdin, so a voter's text never becomes a curl option.
    let tricky = "-o C:\\x.txt --url http://evil.example \"quoted\" ünïcode <b>";
    let before = server.count();
    let voted = vote_with(&net, true, 7, &[11], tricky);
    let last = server.got.lock().unwrap().last().cloned().unwrap_or_default();
    let sent: serde_json::Value = serde_json::from_str(&last.2).unwrap_or_default();
    check("a vote is sent as JSON with the poll, the options, the install number and the comment", last.0 == "POST" && last.1 == "/v1/vote" && sent["poll"] == 7 && sent["options"] == serde_json::json!([11]) && sent["install"] == id.as_str() && sent["comment"] == tricky && server.count() == before + 1);
    check("the vote's answer carries the results", voted.as_ref().is_ok_and(|p| p.voted == Some(vec![11]) && p.results.as_ref().is_some_and(|r| r.total == 3 && r.counts.get("11") == Some(&3))));
    check("an ended poll: a plain error", vote_with(&net, true, 99, &[11], "").err().as_deref() == Some("This poll has ended."));
    let before = server.count();
    check("no answer picked or a comment too long: refused before anything is sent", vote_with(&net, true, 7, &[], "").is_err() && vote_with(&net, true, 7, &[11], &"x".repeat(MAX_COMMENT + 1)).is_err() && server.count() == before);

    // A broken answer.
    server.set(Mode::Broken);
    let broken = list_with(&net, true);
    check("a broken answer is an error, not a crash", broken.as_ref().is_err_and(|e| e.contains("could not read")));
    check("a broken answer to a vote is a plain error", vote_with(&net, true, 7, &[11], "").is_err());

    // A server that hangs: the time limit holds.
    server.set(Mode::Hang);
    let started = std::time::Instant::now();
    let hung = list_with(&net, true);
    let secs = started.elapsed().as_secs_f32();
    check(&format!("a server that hangs: given up after {secs:.1} s (limit {} s)", net.max_time), hung.is_err() && secs < net.max_time as f32 + 2.5);
    let started = std::time::Instant::now();
    let hung_vote = vote_with(&net, true, 7, &[11], "");
    check("a vote to a server that hangs gives up in time", hung_vote.is_err() && started.elapsed().as_secs_f32() < net.max_time as f32 + 2.5);
    // An answer that is too large and has no Content-Length: curl is stopped once 256 KB arrived.
    server.set(Mode::Big);
    let started = std::time::Instant::now();
    let big = list_with(&net, true);
    let secs = started.elapsed().as_secs_f32();
    check(&format!("2 MB sent slowly without a length: stopped after 256 KB in {secs:.1} s ({:?})", big.as_ref().err()), big.as_ref().is_err_and(|e| e.contains("too large")) && secs < 1.0); // curl's own 1 MB stop would take 1.6 s
    server.set(Mode::Normal);

    // The address file: empty, or not https.
    forget_address();
    *server.address.lock().unwrap() = String::new();
    let before = server.count();
    check("an empty address file: no polls, no error, nothing else asked", list_with(&net, true).is_ok_and(|l| l.is_empty()) && server.count() == before + 1);
    forget_address();
    *server.address.lock().unwrap() = "http://polls.example.com".into();
    let before = server.count();
    let not_https = list_with(&net, true);
    check("an address that is not https is not used", not_https.is_err() && server.count() == before + 1);
    forget_address();
    *server.address.lock().unwrap() = "https://polls.example.com".into();
    check("in the test nothing but 127.0.0.1 is ever asked", list_with(&net, true).is_err() && server.got.lock().unwrap().iter().all(|r| r.1 == "/address.txt" || r.1.starts_with("/v1/")));

    // The log holds no install number.
    let log = fs::read_to_string(util::data_dir().join("glint.log")).unwrap_or_default();
    check("the log does not hold the install number", !log.contains(&id) && !log.contains(&a));
    drop(check);
    let _ = fs::write(report_path, lines.join("\n") + "\n");
}
