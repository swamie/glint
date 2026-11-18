//! Share links (optional, off until set up): upload a clip to the user's own
//! Cloudflare Worker (server/share) and copy a link that plays inline in Discord.
//!
//! Setup lives in `settings.share` (Settings → Share links): the Worker's
//! address, its upload key and the name shown on the embed. With no address
//! the feature is hidden in the UI and nothing here contacts any server.
//!
//! One share is a background job of kind "share" (jobs.rs, its own lane):
//! 1. make a Discord-friendly copy in the temp folder: H.264 + AAC MP4 with the
//!    index at the front ("faststart"). An H.264 clip is copied as it is (fast);
//!    HEVC, AV1 or VP9 clips are encoded with libx264. Only the first video and
//!    first audio track (the mix) are kept.
//! 2. grab a 1280-wide JPEG for the embed's picture.
//! 3. upload through the Worker's multipart API in `PART_BYTES` pieces, then the
//!    picture, then complete it. The Worker answers with the link.
//! 4. remember the link in `share.json` (clip id → link and its id), copy it to
//!    the clipboard and emit "clip-shared" { clip, url, copied }.
//!
//! Sharing a clip that already has a link copies that link again (no upload).
//! `share_delete` removes the upload from the Worker and forgets the link.
//!
//! HTTP is done with curl.exe as in update.rs. The upload key never appears on
//! curl's command line: it is written to a header file in the temp folder that
//! curl reads with `-H @file`, and deleted afterwards.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::library::{self, ClipMeta};
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Size of one uploaded piece. R2 wants every piece but the last the same size, at least 5 MiB; the
/// Worker takes request bodies up to 100 MB on the free plan.
pub const PART_BYTES: u64 = 16 * 1024 * 1024;
/// Longest one piece may take to upload.
const PART_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest a small API call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct ShareSettings {
    /// The Worker's address, e.g. `https://glint-share.name.workers.dev`. Empty = feature off.
    pub url: String,
    /// The Worker's `UPLOAD_TOKEN` secret.
    pub token: String,
    /// Shown on the embed as "<name> · <game>". Empty = the game alone.
    pub name: String,
    /// Copy the link when a share finishes.
    pub copy: bool,
}

impl ShareSettings {
    /// The address without a trailing slash, checked: https only (http for 127.0.0.1 and localhost, for testing).
    pub fn base(&self) -> Result<String, String> {
        let u = self.url.trim().trim_end_matches('/').to_string();
        if u.is_empty() {
            return Err("Share links are not set up. Add your Worker's address in Settings → Share links.".into());
        }
        let local = u.starts_with("http://127.0.0.1") || u.starts_with("http://localhost");
        if !u.starts_with("https://") && !local {
            return Err("The share address must start with https://".into());
        }
        if u.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
            return Err("The share address has spaces or quotes in it".into());
        }
        if self.token.trim().is_empty() {
            return Err("Add the upload key in Settings → Share links".into());
        }
        Ok(u)
    }
}

/// A clip's link, kept in share.json.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Shared {
    /// The Worker's id for the upload.
    pub id: String,
    pub url: String,
    /// The Worker's address it was uploaded to (a link is forgotten when that changes).
    pub base: String,
    /// Unix milliseconds.
    pub created: u64,
    pub size: u64,
}

static LINKS: Mutex<Option<BTreeMap<String, Shared>>> = Mutex::new(None);

fn links_file() -> PathBuf {
    util::data_dir().join("share.json")
}

fn with_links<R>(f: impl FnOnce(&mut BTreeMap<String, Shared>) -> R) -> R {
    let mut g = LINKS.lock().unwrap();
    let map = g.get_or_insert_with(|| fs::read_to_string(links_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(map)
}

fn save_links(map: &BTreeMap<String, Shared>) {
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let tmp = links_file().with_extension("json.tmp");
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, links_file());
        }
    }
}

/// The link of clip `id` on the Worker at `base`, if it has one.
pub fn link_of(id: &str, base: &str) -> Option<Shared> {
    with_links(|m| m.get(id).filter(|s| s.base == base).cloned())
}

/* ---------- curl ---------- */

/// What the Worker answered.
struct Answer {
    code: u16,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    /// The Worker's own reason ({"error": "..."}), or the status.
    fn why(&self) -> String {
        match self.json().get("error").and_then(Value::as_str) {
            Some(e) => e.to_string(),
            None if self.code == 401 => "The upload key was not accepted. Check it in Settings → Share links.".into(),
            None if self.code == 404 => "The share address did not answer as a Glint share Worker. Check it in Settings → Share links.".into(),
            None => format!("The share server answered {}", self.code),
        }
    }
}

enum Body<'a> {
    None,
    Json(&'a Value),
    /// A file sent as the body (PUT, streamed by curl).
    File(&'a Path, &'a str),
}

/// One HTTP call to the Worker with curl.exe. Cancelling kills curl.
fn call(method: &str, url: &str, token: &str, body: Body, limit: Duration, cancel: Option<&AtomicBool>) -> Result<Answer, String> {
    let dir = util::tmp_dir();
    let tag = util::new_id();
    let hdr = dir.join(format!("share-{tag}.hdr"));
    let json_file = dir.join(format!("share-{tag}.json"));
    let result = (|| {
        // The key goes in a file curl reads, never on its command line.
        {
            let mut f = fs::File::create(&hdr).map_err(|e| format!("Could not prepare the upload: {e}"))?;
            writeln!(f, "Authorization: Bearer {}", token.trim()).map_err(|e| e.to_string())?;
        }
        let mut cmd = util::hidden_cmd(Path::new("curl.exe"));
        cmd.current_dir(&dir)
            .args(["-sS", "--max-time", &limit.as_secs().to_string(), "-A", "Glint", "-H", "Expect:", "-o", "-", "-w", "\n%{http_code}", "-X", method])
            .arg("-H")
            .arg(format!("@{}", library::file_name(&hdr)));
        match body {
            Body::None => {}
            Body::Json(v) => {
                fs::write(&json_file, v.to_string()).map_err(|e| format!("Could not prepare the upload: {e}"))?;
                cmd.args(["-H", "Content-Type: application/json", "--data-binary"]).arg(format!("@{}", library::file_name(&json_file)));
            }
            Body::File(p, ctype) => {
                // Named relative to the temp folder (curl's working folder), like the other files,
                // so a profile path with non-English letters never reaches curl's command line.
                let name = if p.parent() == Some(dir.as_path()) { PathBuf::from(library::file_name(p)) } else { p.to_path_buf() };
                cmd.arg("-H").arg(format!("Content-Type: {ctype}")).arg("-T").arg(name);
            }
        }
        let mut child = cmd.arg(url).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not run curl.exe: {e}"))?;
        let mut so = child.stdout.take();
        let mut se = child.stderr.take();
        let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = so.as_mut() { let _ = s.read_to_end(&mut b); } b });
        let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
        let cancelled = loop {
            match child.try_wait() {
                Ok(Some(_)) => break false,
                Ok(None) if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break true;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("curl.exe: {e}")),
            }
        };
        let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).to_string();
        let stderr = t_err.join().unwrap_or_default();
        if cancelled {
            return Err("Cancelled".into());
        }
        let (answer, code) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_str()));
        let code: u16 = code.trim().parse().unwrap_or(0);
        if code == 0 {
            let why = util::last_lines(&stderr, 1);
            return Err(format!("Could not reach the share server. Check the internet connection and the address. ({})", if why.is_empty() { "no answer".into() } else { why }));
        }
        Ok(Answer { code, body: answer.to_string() })
    })();
    let _ = fs::remove_file(&hdr);
    let _ = fs::remove_file(&json_file);
    result
}

/* ---------- making the file ---------- */

/// Codec names of the first video and audio streams ("" when there is none).
fn codecs(ffmpeg: &Path, file: &Path) -> (String, String, String) {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Default::default() };
    let mut cmd = util::hidden_cmd(&ffprobe);
    cmd.args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,pix_fmt", "-of", "default=nw=1"]).arg(library::long_path(file));
    let out = library::run_limited(&mut cmd, library::PROBE_TIMEOUT);
    let (mut v, mut pix, mut a) = (String::new(), String::new(), String::new());
    let mut kind = String::new();
    let mut name = String::new();
    let mut fmt = String::new();
    let mut flush = |kind: &str, name: &str, fmt: &str| {
        if kind == "video" && v.is_empty() {
            v = name.to_string();
            pix = fmt.to_string();
        } else if kind == "audio" && a.is_empty() {
            a = name.to_string();
        }
    };
    // `default=nw=1` prints each stream's keys in the order asked: codec_name, codec_type, pix_fmt.
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((k, val)) = line.split_once('=') else { continue };
        match k {
            "codec_name" => {
                if !kind.is_empty() {
                    flush(&kind, &name, &fmt);
                }
                name = val.into();
                kind.clear();
                fmt.clear();
            }
            "codec_type" => kind = val.into(),
            "pix_fmt" => fmt = val.into(),
            _ => {}
        }
    }
    if !kind.is_empty() {
        flush(&kind, &name, &fmt);
    }
    (v, pix, a)
}

/// The ffmpeg arguments that turn `src` into a file Discord plays inline.
/// H.264 4:2:0 video is copied; anything else is encoded. Audio is copied when it is AAC.
pub fn prepare_args(src: &Path, out: &Path, vcodec: &str, pix: &str, acodec: &str, fps: u32) -> Vec<String> {
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(library::long_path(src).to_string_lossy().into_owned());
    a.extend(["-map", "0:v:0", "-map", "0:a:0?", "-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1"].iter().map(|s| s.to_string()));
    if vcodec == "h264" && (pix.is_empty() || pix == "yuv420p" || pix == "yuvj420p") {
        a.extend(["-c:v", "copy"].iter().map(|s| s.to_string()));
    } else {
        // At most 1080p60: Discord's player gains nothing above it and the file stays small.
        let vf = if fps > 60 { "scale='min(1920,iw)':-2,fps=60" } else { "scale='min(1920,iw)':-2" };
        a.extend(["-vf", vf, "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-profile:v", "high", "-pix_fmt", "yuv420p"].iter().map(|s| s.to_string()));
    }
    if acodec == "aac" {
        a.extend(["-c:a", "copy"].iter().map(|s| s.to_string()));
    } else {
        a.extend(["-c:a", "aac", "-b:a", "160k", "-ac", "2"].iter().map(|s| s.to_string()));
    }
    a.extend(["-movflags", "+faststart", "-f", "mp4"].iter().map(|s| s.to_string()));
    a.push(out.to_string_lossy().into_owned());
    a
}

/// Run ffmpeg with `args`, killing it on cancel.
fn run_ffmpeg(ffmpeg: &Path, args: &[String], cancel: &AtomicBool) -> Result<(), String> {
    let mut child = util::hidden_cmd(ffmpeg).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not run ffmpeg: {e}"))?;
    let mut se = child.stderr.take();
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if cancel.load(Ordering::SeqCst) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Cancelled".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let err = t_err.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("ffmpeg could not prepare the clip: {}", util::last_lines(&err, 2)));
    }
    Ok(())
}

/// The embed's picture: one frame from the middle, 1280 wide at most.
fn make_poster(ffmpeg: &Path, video: &Path, at: f64, out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let args: Vec<String> = vec![
        "-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into(),
        "-ss".into(), format!("{at:.2}"), "-i".into(), video.to_string_lossy().into_owned(),
        "-frames:v".into(), "1".into(),
        "-vf".into(), "scale='min(1280,iw)':-2".into(),
        "-q:v".into(), "3".into(), out.to_string_lossy().into_owned(),
    ];
    run_ffmpeg(ffmpeg, &args, cancel)
}

/* ---------- the job ---------- */

/// What the UI gets from `share_clip`: the link at once when the clip already has one, else the queued job.
#[derive(Serialize)]
pub struct Started {
    pub url: String,
    pub copied: bool,
    pub job: Option<Job>,
}

fn copy_text(text: &str) -> bool {
    use clipboard_win::{formats, Clipboard, Setter};
    let Ok(_clip) = Clipboard::new_attempts(10) else { return false };
    formats::Unicode.write_clipboard(&text).is_ok()
}

pub fn enqueue(core: &Arc<Core>, clip_id: String) -> Result<Started, String> {
    let s = core.settings();
    let base = s.share.base()?;
    let meta = core.lib.lock().unwrap().get_saved(&clip_id)?;
    if let Some(old) = link_of(&clip_id, &base) {
        let copied = copy_text(&old.url);
        return Ok(Started { url: old.url, copied, job: None });
    }
    if !Path::new(&meta.path).is_file() {
        return Err("The clip's video file is missing".into());
    }
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let settings = s.share.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let core = weak.upgrade().ok_or("Glint is closing")?;
        let link = upload(&ffmpeg, &meta, &settings, &base, cancel, &mut |p| progress(p))?;
        with_links(|m| {
            m.insert(meta.id.clone(), link.clone());
            save_links(m);
        });
        let copied = settings.copy && copy_text(&link.url);
        core.emit("clip-shared", json!({ "clip": meta.id, "url": link.url, "copied": copied }));
        Ok(meta.id.clone())
    });
    let job = core.jobs.enqueue(&clip_id, &meta_title(core, &clip_id), "share", run)?;
    Ok(Started { url: String::new(), copied: false, job: Some(job) })
}

fn meta_title(core: &Arc<Core>, id: &str) -> String {
    core.lib.lock().unwrap().get(id).map(|m| m.title).unwrap_or_default()
}

/// Prepare and upload one clip. Progress: 0..15 preparing, 15..100 uploading.
fn upload(ffmpeg: &Path, meta: &ClipMeta, cfg: &ShareSettings, base: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<Shared, String> {
    let started = Instant::now();
    let tmp = util::tmp_dir();
    let tag = format!("share-{}", util::new_id());
    let video = tmp.join(format!("{tag}.mp4"));
    let poster = tmp.join(format!("{tag}.jpg"));
    let result = (|| {
        let src = Path::new(&meta.path);
        let (v, pix, a) = codecs(ffmpeg, src);
        progress(1.0);
        run_ffmpeg(ffmpeg, &prepare_args(src, &video, &v, &pix, &a, meta.fps), cancel)?;
        progress(12.0);
        let probe = library::probe(ffmpeg, &video, false).unwrap_or_default();
        let duration = if probe.duration > 0.0 { probe.duration } else { meta.duration };
        make_poster(ffmpeg, &video, library::thumb_at(duration), &poster, cancel)?;
        progress(15.0);
        let size = fs::metadata(&video).map(|m| m.len()).map_err(|e| e.to_string())?;
        let token = cfg.token.as_str();

        // 1. Start: the Worker checks the space left and gives the upload its id.
        let info = json!({
            "title": meta.title, "uploader": cfg.name.trim(), "game": meta.game,
            "width": if probe.width > 0 { probe.width } else { meta.width },
            "height": if probe.height > 0 { probe.height } else { meta.height },
            "duration": (duration * 100.0).round() / 100.0, "size": size,
        });
        let ans = call("POST", &format!("{base}/api/uploads"), token, Body::Json(&info), CALL_TIMEOUT, Some(cancel))?;
        if ans.code != 200 {
            return Err(ans.why());
        }
        let j = ans.json();
        let id = j.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let upload_id = j.get("uploadId").and_then(Value::as_str).unwrap_or_default().to_string();
        if id.is_empty() || upload_id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("The share server gave an answer Glint does not understand. Is the Worker up to date?".into());
        }
        let abort = || {
            let _ = call("DELETE", &format!("{base}/api/uploads/{id}?uploadId={}", enc(&upload_id)), token, Body::None, CALL_TIMEOUT, None);
        };

        // 2. The pieces, each read into its own temp file so curl can stream it.
        let pieces = size.div_ceil(PART_BYTES).max(1);
        let mut parts = Vec::new();
        let mut file = fs::File::open(&video).map_err(|e| e.to_string())?;
        for n in 1..=pieces {
            let piece = tmp.join(format!("{tag}.part"));
            let from = (n - 1) * PART_BYTES;
            let len = PART_BYTES.min(size - from);
            let res = (|| {
                file.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
                let mut buf = vec![0u8; len as usize];
                file.read_exact(&mut buf).map_err(|e| e.to_string())?;
                fs::write(&piece, &buf).map_err(|e| format!("Could not prepare the upload: {e}"))?;
                drop(buf);
                let url = format!("{base}/api/uploads/{id}/parts/{n}?uploadId={}", enc(&upload_id));
                // One retry for a dropped connection.
                let mut ans = call("PUT", &url, token, Body::File(&piece, "application/octet-stream"), PART_TIMEOUT, Some(cancel));
                if ans.as_ref().is_err_and(|e| e != "Cancelled") {
                    ans = call("PUT", &url, token, Body::File(&piece, "application/octet-stream"), PART_TIMEOUT, Some(cancel));
                }
                let ans = ans?;
                if ans.code != 200 {
                    return Err(ans.why());
                }
                ans.json().get("etag").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "The share server did not confirm a piece of the upload".to_string())
            })();
            let _ = fs::remove_file(&piece);
            match res {
                Ok(etag) => parts.push(json!({ "partNumber": n, "etag": etag })),
                Err(e) => {
                    abort();
                    return Err(e);
                }
            }
            progress(15.0 + 80.0 * (n as f32 / pieces as f32));
        }

        // 3. The picture, then finish.
        let ans = call("PUT", &format!("{base}/api/uploads/{id}/poster?uploadId={}", enc(&upload_id)), token, Body::File(&poster, "image/jpeg"), CALL_TIMEOUT, Some(cancel));
        match ans {
            Ok(a) if a.code == 200 => {}
            Ok(a) => {
                abort();
                return Err(a.why());
            }
            Err(e) => {
                abort();
                return Err(e);
            }
        }
        let ans = call("POST", &format!("{base}/api/uploads/{id}/complete?uploadId={}", enc(&upload_id)), token, Body::Json(&json!({ "parts": parts })), CALL_TIMEOUT, Some(cancel))?;
        if ans.code != 200 {
            abort();
            return Err(ans.why());
        }
        let url = ans.json().get("url").and_then(Value::as_str).unwrap_or_default().to_string();
        if !url.starts_with("http") {
            return Err("The share server did not send the link back".into());
        }
        progress(100.0);
        util::log(&format!("share: clip {} uploaded ({} bytes, {} pieces) in {:.1} s", meta.id, size, pieces, started.elapsed().as_secs_f32()));
        Ok(Shared { id, url, base: base.to_string(), created: util::unix_ms(), size })
    })();
    let _ = fs::remove_file(&video);
    let _ = fs::remove_file(&poster);
    result
}

/// Percent-encode a query value (the Worker's upload ids are base64-like).
fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/* ---------- commands ---------- */

type St<'a> = tauri::State<'a, Arc<Core>>;

/// Share clip `id`: copies its link at once when it already has one, else queues the upload.
#[tauri::command(async)]
pub fn share_clip(core: St, id: String) -> Result<Started, String> {
    enqueue(core.inner(), id)
}

/// The links of these clips on the current Worker: clip id → link.
#[tauri::command(async)]
pub fn share_links(core: St) -> BTreeMap<String, String> {
    let Ok(base) = core.settings().share.base() else { return BTreeMap::new() };
    with_links(|m| m.iter().filter(|(_, s)| s.base == base).map(|(k, s)| (k.clone(), s.url.clone())).collect())
}

/// Copy a text (a link) to the clipboard.
#[tauri::command(async)]
pub fn share_copy(text: String) -> bool {
    copy_text(&text)
}

/// Delete clip `id`'s upload from the Worker and forget its link.
#[tauri::command(async)]
pub fn share_delete(core: St, id: String) -> Result<(), String> {
    let s = core.settings().share;
    let base = s.base()?;
    let Some(link) = link_of(&id, &base) else { return Ok(()) };
    let ans = call("DELETE", &format!("{base}/api/clips/{}", link.id), &s.token, Body::None, CALL_TIMEOUT, None)?;
    if ans.code != 200 && ans.code != 404 {
        return Err(ans.why());
    }
    with_links(|m| {
        m.remove(&id);
        save_links(m);
    });
    Ok(())
}

/// Check the address and key: { used, limit, clips } bytes from the Worker.
#[tauri::command(async)]
pub fn share_test(core: St) -> Result<Value, String> {
    let s = core.settings().share;
    let base = s.base()?;
    let ans = call("GET", &format!("{base}/api/usage"), &s.token, Body::None, CALL_TIMEOUT, None)?;
    if ans.code != 200 {
        return Err(ans.why());
    }
    Ok(ans.json())
}
