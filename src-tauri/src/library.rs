//! Clip index (library.json) plus ffprobe/thumbnail helpers.
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ClipMeta {
    pub id: String,
    pub path: String,
    pub title: String,
    pub game: String,
    /// Unix milliseconds.
    pub created: u64,
    pub duration: f64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub favorite: bool,
    /// "clip" | "trim" | "edit" | "blur" | "import" | "discord" (a copy compressed for Discord)
    pub kind: String,
    pub thumb: String,
    /// 3 = mix + desktop + mic, 1 = single track, 0 = silent.
    pub audio_tracks: u32,
    /// Subfolder of the clips folder that holds the file. "" = the clips folder itself.
    pub folder: String,
    /// Kind "joined" ("Edit together", join.rs): where each joined clip after the first starts, seconds.
    /// The editor opens the clip with a split at each. Not written for other clips.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub joins: Vec<f64>,
    /// Kind "joined": the title of each joined clip, in order (`joins.len() + 1` names).
    /// The editor shows each as a clip of its own that can be moved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub join_names: Vec<String>,
    /// A working file of "Edit together" / an editor project (projects.rs, id = the project's):
    /// never shown in the library, deleted with its project (and at the next start when it
    /// belongs to none). What the editor exports from it is a normal clip.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
    /// Share links (share.rs): the file was deleted from this PC after its upload was checked; the
    /// clip stays in the library and can be downloaded again. Such a clip has no file at `path`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cloud_only: bool,
}

pub struct Library {
    pub clips: Vec<ClipMeta>,
    /// Batches open right now (`batch`): while there is one, `save` only notes that there is something to write.
    hold: u32,
    dirty: bool,
}

/// While this lives, changes to the library are written once, when it is dropped, not once per clip
/// (deleting, moving or renaming many clips wrote the whole of library.json for each of them).
pub struct Batch<'a>(&'a std::sync::Mutex<Library>);

/// Open a batch on `lib`. The lock is not held in between: only the writing waits.
pub fn batch(lib: &std::sync::Mutex<Library>) -> Batch<'_> {
    lib.lock().unwrap_or_else(|e| e.into_inner()).hold += 1;
    Batch(lib)
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        let mut l = self.0.lock().unwrap_or_else(|e| e.into_inner());
        l.hold = l.hold.saturating_sub(1);
        if l.hold == 0 && l.dirty {
            l.save();
        }
    }
}

impl Library {
    fn file() -> PathBuf {
        util::data_dir().join("library.json")
    }

    pub fn load() -> Self {
        let mut clips: Vec<ClipMeta> = fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        clips.retain(|c| c.cloud_only || Path::new(&c.path).is_file());
        Library { clips, hold: 0, dirty: false }
    }

    pub fn save(&mut self) {
        if self.hold > 0 {
            self.dirty = true; // written when the batch ends
            return;
        }
        self.dirty = false;
        if let Ok(t) = serde_json::to_string(&self.clips) {
            let _ = util::write_atomic(&Self::file(), t);
        }
    }

    pub fn add(&mut self, meta: ClipMeta) {
        self.clips.retain(|c| c.id != meta.id);
        self.clips.push(meta);
        self.save();
    }

    pub fn get(&self, id: &str) -> Option<ClipMeta> {
        self.clips.iter().find(|c| c.id == id).cloned()
    }

    /// The clips the user sees: every clip but the drafts of "Edit together".
    pub fn visible(&self) -> Vec<ClipMeta> {
        self.clips.iter().filter(|c| !c.draft).cloned().collect()
    }

    /// Clip `id` for a command that makes no sense on a draft (rename, move,
    /// favourite, delete, copy, compress, Blur): a draft is refused with `DRAFT_ERR`.
    pub fn get_saved(&self, id: &str) -> Result<ClipMeta, String> {
        match self.get(id) {
            Some(c) if c.draft => Err(DRAFT_ERR.into()),
            Some(c) => Ok(c),
            None => Err("Clip not found".into()),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<ClipMeta> {
        let pos = self.clips.iter().position(|c| c.id == id)?;
        let meta = self.clips.remove(pos);
        self.save();
        Some(meta)
    }

    /// Give every clip the one name its game is filed under (see `games::canonical`).
    /// Returns how many clips changed.
    pub fn retag_games(&mut self) -> usize {
        let mut n = 0;
        for c in &mut self.clips {
            let to = crate::games::canonical(&c.game);
            if to != c.game {
                c.game = to;
                n += 1;
            }
        }
        if n > 0 {
            self.save();
        }
        n
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut ClipMeta)) -> Option<ClipMeta> {
        let c = self.clips.iter_mut().find(|c| c.id == id)?;
        f(c);
        let out = c.clone();
        self.save();
        Some(out)
    }
}

pub fn thumbs_dir() -> PathBuf {
    let d = util::data_dir().join("thumbs");
    let _ = fs::create_dir_all(&d);
    d
}

pub fn ffprobe_for(ffmpeg: &Path) -> Option<PathBuf> {
    let sibling = ffmpeg.with_file_name("ffprobe.exe");
    if sibling.is_file() {
        return Some(sibling);
    }
    util::find_exe("", "ffprobe")
}

#[derive(Default, Debug, Clone, Serialize)]
pub struct Probe {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub video_streams: u32,
    pub audio_streams: u32,
}

/// Read stream layout and duration. `count_frames` also counts video packets (slower).
pub fn probe(ffmpeg: &Path, file: &Path, count_frames: bool) -> Option<Probe> {
    // One export asked for the same file two or three times, each an ffprobe run. An answer is
    // kept for half a minute, for the file as it is now (same size and time of change).
    type Key = (PathBuf, bool, u64, Option<std::time::SystemTime>);
    static SEEN: std::sync::Mutex<Vec<(Key, Probe, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    let key: Option<Key> = fs::metadata(file).ok().map(|m| (file.to_path_buf(), count_frames, m.len(), m.modified().ok()));
    if let Some(key) = &key {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|(_, _, at)| at.elapsed() < std::time::Duration::from_secs(30));
        if let Some((_, p, _)) = seen.iter().find(|(k, _, _)| k == key) {
            return Some(p.clone());
        }
    }
    let p = probe_now(ffmpeg, file, count_frames)?;
    // A file that is still being written (a changed size or time by now) is not remembered.
    if let Some(key) = key.filter(|k| fs::metadata(file).ok().is_some_and(|m| m.len() == k.2 && m.modified().ok() == k.3) && p.duration > 0.0) {
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= 64 {
            seen.remove(0);
        }
        seen.push((key, p.clone(), std::time::Instant::now()));
    }
    Some(p)
}

fn probe_now(ffmpeg: &Path, file: &Path, count_frames: bool) -> Option<Probe> {
    let ffprobe = ffprobe_for(ffmpeg)?;
    let mut cmd = util::hidden_cmd(&ffprobe);
    cmd.args(["-v", "error"]);
    if count_frames {
        cmd.arg("-count_packets");
    }
    cmd.args(["-show_entries", "stream=codec_type,width,height,avg_frame_rate,nb_read_packets:format=duration", "-of", "default=nw=1"]);
    // thumbs-refresh (0.1.46): a long path gets the \\?\ form, and a file that does not answer (a sleeping network
    // drive, a cloud placeholder) ends the probe after PROBE_TIMEOUT instead of holding up everything after it.
    cmd.arg(long_path(file));
    let out = run_limited(&mut cmd, PROBE_TIMEOUT);
    if out.start_error.is_some() {
        return None;
    }
    if out.timed_out {
        util::log(&format!("probe: {} did not answer within {} s", file_name(file), PROBE_TIMEOUT.as_secs()));
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut p = Probe::default();
    let mut in_video = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "codec_type" => {
                in_video = v == "video";
                if in_video {
                    p.video_streams += 1;
                } else if v == "audio" {
                    p.audio_streams += 1;
                }
            }
            "width" if in_video && p.width == 0 => p.width = v.parse().unwrap_or(0),
            "height" if in_video && p.height == 0 => p.height = v.parse().unwrap_or(0),
            "avg_frame_rate" if in_video && p.fps == 0 => {
                if let Some((n, d)) = v.split_once('/') {
                    let (n, d): (f64, f64) = (n.parse().unwrap_or(0.0), d.parse().unwrap_or(0.0));
                    if d > 0.0 {
                        p.fps = (n / d).round() as u32;
                    }
                }
            }
            "nb_read_packets" if in_video && p.frames == 0 => p.frames = v.parse().unwrap_or(0),
            "duration" => p.duration = v.parse().unwrap_or(p.duration),
            _ => {}
        }
    }
    Some(p)
}

/// The thumbnail filter: first turn non-square pixels square (an anamorphic or
/// "stretched" import stores 1440x1080 with a 4:3 sample aspect, which a JPEG
/// cannot carry, so a plain `scale=480:-2` came out squashed), then fit the
/// whole frame inside 640x640 without cropping. Rotation is applied by ffmpeg.
pub const THUMB_VF: &str = "scale=w='trunc(iw*sar/2)*2':h=ih,setsar=1,scale=640:640:force_original_aspect_ratio=decrease:force_divisible_by=2,setsar=1";

/// Where in a clip the thumbnail is taken: the middle.
pub fn thumb_at(duration: f64) -> f64 {
    if duration.is_finite() && duration > 0.0 { duration * 0.5 } else { 0.0 }
}

/// Grab one frame as the library thumbnail. Returns the jpg path ("" on failure).
pub fn make_thumb(ffmpeg: &Path, video: &Path, id: &str, at: f64) -> String {
    make_thumb_why(ffmpeg, video, id, at).0
}

/// Clips whose failed thumbnail was already logged this session (once per clip).
static THUMB_LOGGED: std::sync::Mutex<Option<std::collections::HashSet<String>>> = std::sync::Mutex::new(None);

/// `make_thumb`, with ffmpeg's last words when every attempt failed: (jpg path or "", error).
pub fn make_thumb_why(ffmpeg: &Path, video: &Path, id: &str, at: f64) -> (String, String) {
    let t = make_thumb_full(ffmpeg, video, id, at);
    if t.path.is_empty() {
        let first = THUMB_LOGGED.lock().map(|mut g| g.get_or_insert_with(Default::default).insert(id.split("-r").next().unwrap_or(id).to_string())).unwrap_or(true);
        if first {
            util::log(&format!("thumbnail: none for clip {id} ({}): {}", file_name(video), t.log_line()));
        }
    }
    (t.path, t.error)
}

/* ---------- thumbs-refresh (0.1.46): a thumbnail that cannot hang, and says why it failed ---------- */

/// Longest an ffprobe run may take (a sleeping network drive, a cloud placeholder file: it is ended after that).
pub const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest one thumbnail attempt may take.
pub const THUMB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What `run_limited` got from a child process.
#[derive(Debug, Default)]
pub struct Ran {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The exit code (None: killed after the time limit, or never started).
    pub code: Option<i32>,
    pub timed_out: bool,
    pub start_error: Option<String>,
}

/// Run `cmd` (stdin closed, stdout and stderr read) and end it if it is not done within `limit`.
/// A hanging ffmpeg or ffprobe used to hold up every thumbnail after it (they are made one at a time).
pub fn run_limited(cmd: &mut std::process::Command, limit: std::time::Duration) -> Ran {
    use std::io::Read;
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return Ran { start_error: Some(e.to_string()), ..Default::default() },
    };
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = so.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let until = std::time::Instant::now() + limit;
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) if std::time::Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(15)),
            Err(_) => break None,
        }
    };
    Ran { stdout: t_out.join().unwrap_or_default(), stderr: t_err.join().unwrap_or_default(), code, timed_out, start_error: None }
}

/// A path ffmpeg can open even when it is longer than Windows' old 260-character limit: such a path is given in
/// the `\\?\` form (ffmpeg opens files with the wide Windows calls, which take it). Short paths stay as they are.
pub fn long_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.len() < 240 || s.starts_with(r"\\?\") || !p.is_absolute() {
        return p.to_path_buf();
    }
    match s.strip_prefix(r"\\") {
        Some(unc) => PathBuf::from(format!(r"\\?\UNC\{unc}")),
        None => PathBuf::from(format!(r"\\?\{}", s.replace('/', "\\"))),
    }
}

/// A file's name without its folders (what the log may say about a clip).
pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// What `make_thumb_full` did.
#[derive(Debug, Default, Clone)]
pub struct Thumb {
    /// The jpg ("" when none could be made).
    pub path: String,
    /// Why not: the last line ffmpeg said (or Glint's own reason).
    pub error: String,
    /// ffmpeg's exit code of the last attempt (None: killed after THUMB_TIMEOUT, or not started).
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// The attempt that worked (1..=4), 0 when none did.
    pub attempt: usize,
}

impl Thumb {
    /// For glint.log: "exit 1: <last line>", "timed out after 30 s", ...
    pub fn log_line(&self) -> String {
        if self.timed_out {
            return format!("timed out after {} s", THUMB_TIMEOUT.as_secs());
        }
        match self.exit {
            Some(c) => format!("ffmpeg exit {c}: {}", self.error),
            None => self.error.clone(),
        }
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// The thumbnail of `video` (the frame at `at` seconds) as `<thumbs>\<id>.jpg`.
///
/// The fallback chain, each attempt software decoding only, at below-normal priority, ended after THUMB_TIMEOUT:
/// 1. the wanted time (`-ss at` before the input);
/// 2. 0 s (a wrong length, a seek that lands past the end of a very short or still-growing file);
/// 3. the first key frame (`-skip_frame nokey`: only key frames are decoded, so a broken stretch is stepped over);
/// 4. a plain `scale` and `format=yuvj420p` (the 4:2:0 full-range picture every JPEG encoder takes, for a source
///    whose pixel format, colour tags or odd size trip the usual filter).
/// ffmpeg writes the JPEG to Glint through a pipe and Glint saves it: a thumbnails folder ffmpeg itself may not
/// write to (a security tool that blocks unknown programs from AppData) or an odd folder name no longer matters.
pub fn make_thumb_full(ffmpeg: &Path, video: &Path, id: &str, at: f64) -> Thumb {
    use std::os::windows::process::CommandExt;
    let dir = thumbs_dir();
    if !dir.is_dir() {
        return Thumb { error: format!("the thumbnails folder could not be made ({})", dir.display()), ..Default::default() };
    }
    let out = dir.join(format!("{id}.jpg"));
    let at = if at.is_finite() && at > 0.0 { at } else { 0.0 };
    let input = long_path(video);
    let plain_vf = "scale=640:640:force_original_aspect_ratio=decrease:force_divisible_by=2,format=yuvj420p";
    // (input options, filter, extra output options)
    let tries: [(Vec<String>, &str, &[&str]); 4] = [
        (vec!["-ss".into(), format!("{at:.3}")], THUMB_VF, &[]),
        (vec![], THUMB_VF, &[]),
        (vec!["-skip_frame".into(), "nokey".into()], THUMB_VF, &[]),
        (vec![], plain_vf, &["-strict", "unofficial"]),
    ];
    let mut last = Thumb::default();
    for (k, (pre, vf, extra)) in tries.iter().enumerate() {
        if k == 1 && at <= 0.0 {
            continue; // the first attempt was 0 s already
        }
        let mut cmd = std::process::Command::new(ffmpeg);
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-hwaccel", "none"]).args(pre).arg("-i").arg(&input);
        cmd.args(["-map", "0:v:0", "-frames:v", "1", "-vf", vf]).args(*extra).args(["-c:v", "mjpeg", "-q:v", "5", "-f", "image2pipe", "pipe:1"]);
        let ran = run_limited(&mut cmd, THUMB_TIMEOUT);
        if let Some(e) = ran.start_error {
            return Thumb { error: format!("ffmpeg could not be started ({e})"), ..Default::default() };
        }
        let jpeg = ran.stdout.len() > 100 && ran.stdout.starts_with(&[0xFF, 0xD8]);
        if jpeg {
            let part = out.with_extension("part");
            let saved = fs::write(&part, &ran.stdout).and_then(|_| fs::rename(&part, &out));
            return match saved {
                Ok(()) => Thumb { path: out.to_string_lossy().into_owned(), attempt: k + 1, exit: ran.code, ..Default::default() },
                Err(e) => {
                    let _ = fs::remove_file(&part);
                    Thumb { error: format!("the picture could not be saved in the thumbnails folder ({e})"), exit: ran.code, ..Default::default() }
                }
            };
        }
        let line = String::from_utf8_lossy(&ran.stderr).lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
        last = Thumb { error: if line.is_empty() { "no picture came out".into() } else { line }, exit: ran.code, timed_out: ran.timed_out, ..Default::default() };
        if ran.timed_out {
            break; // a file that does not answer will not answer the next attempt either
        }
    }
    last
}

/// Build a library entry for a video file that already exists on disk.
pub fn register(ffmpeg: &Path, file: &Path, title: &str, game: &str, kind: &str) -> ClipMeta {
    register_id(ffmpeg, file, util::new_id(), title, game, kind)
}

/// `register` with the id chosen by the caller (a draft's file is named after its id).
pub fn register_id(ffmpeg: &Path, file: &Path, id: String, title: &str, game: &str, kind: &str) -> ClipMeta {
    let p = probe(ffmpeg, file, false).unwrap_or_default();
    ClipMeta {
        thumb: make_thumb(ffmpeg, file, &id, thumb_at(p.duration)),
        id,
        path: file.to_string_lossy().into_owned(),
        title: title.to_string(),
        game: game.to_string(),
        created: util::unix_ms(),
        duration: p.duration,
        size: fs::metadata(file).map(|m| m.len()).unwrap_or(0),
        width: p.width,
        height: p.height,
        fps: p.fps,
        favorite: false,
        kind: kind.to_string(),
        audio_tracks: p.audio_streams,
        folder: String::new(),
        joins: Vec::new(),
        join_names: Vec::new(),
        draft: false,
        cloud_only: false,
    }
}

// ---------------------------------------------------------------------------
// Drafts of "Edit together" (join.rs)

/// The folder inside the clips folder that holds drafts. Its name starts with a
/// dot, so the folder list, rescan and import never look inside it.
pub const DRAFTS: &str = ".drafts";

/// What a command that makes no sense on a draft answers.
pub const DRAFT_ERR: &str = "This clip is not saved yet. Export it from the editor first";

/// `<clips_dir>\.drafts`, created and marked hidden.
pub fn drafts_dir(clips_dir: &Path) -> PathBuf {
    let d = clips_dir.join(DRAFTS);
    if !d.is_dir() {
        let _ = fs::create_dir_all(&d);
        use windows::Win32::Storage::FileSystem::{GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_FLAGS_AND_ATTRIBUTES};
        let name = windows::core::HSTRING::from(d.as_os_str());
        // SAFETY: plain Win32 calls on a valid, NUL-terminated wide string.
        unsafe {
            let now = GetFileAttributesW(&name);
            if now != u32::MAX {
                let _ = SetFileAttributesW(&name, FILE_FLAGS_AND_ATTRIBUTES(now | FILE_ATTRIBUTE_HIDDEN.0));
            }
        }
    }
    d
}

/// True when `file` sits directly in a folder named `.drafts` (the only place
/// a draft's file may be deleted from).
pub fn in_drafts_dir(file: &Path) -> bool {
    file.parent().and_then(|p| p.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(DRAFTS))
}

/// Delete a draft's file (only inside a `.drafts` folder) and its thumbnail
/// (only inside the thumbnails folder). The library entry is the caller's.
pub fn remove_draft_files(meta: &ClipMeta) {
    let file = Path::new(&meta.path);
    if in_drafts_dir(file) {
        let _ = fs::remove_file(file);
    }
    remove_thumb(&meta.thumb);
}

fn remove_thumb(thumb: &str) {
    let t = Path::new(thumb);
    let ours = t.parent().is_some_and(|p| p.to_string_lossy().to_lowercase() == thumbs_dir().to_string_lossy().to_lowercase());
    if !thumb.is_empty() && ours {
        let _ = fs::remove_file(t);
    }
}

/// At start-up: drop every draft entry with its file and thumbnail, and delete
/// every file directly inside `<clips_dir>\.drafts` and inside the `.drafts`
/// folder of any draft entry (the clips folder may have changed since). Nothing
/// outside a folder named `.drafts` is deleted, subfolders and links are left
/// alone, and an empty `.drafts` folder is removed. Returns the files deleted.
/// The drafts whose id is in `keep` (editor projects, projects.rs) are kept:
/// their entry, file and thumbnail (and their `.drafts` folder is still swept of the rest).
pub fn sweep_drafts(lib: &mut Library, clips_dir: &Path, keep: &[String]) -> usize {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if !clips_dir.as_os_str().is_empty() {
        dirs.push(clips_dir.join(DRAFTS));
    }
    let kept: Vec<String> = lib.clips.iter().filter(|c| c.draft && keep.contains(&c.id)).map(|c| c.path.to_lowercase()).collect();
    for c in lib.clips.iter().filter(|c| c.draft && keep.contains(&c.id)) {
        if let Some(dir) = Path::new(&c.path).parent().filter(|_| in_drafts_dir(Path::new(&c.path))) {
            if !dirs.iter().any(|x| x.to_string_lossy().to_lowercase() == dir.to_string_lossy().to_lowercase()) {
                dirs.push(dir.to_path_buf());
            }
        }
    }
    let drafts: Vec<ClipMeta> = lib.clips.iter().filter(|c| c.draft && !keep.contains(&c.id)).cloned().collect();
    for d in &drafts {
        let p = Path::new(&d.path);
        if in_drafts_dir(p) {
            if let Some(dir) = p.parent() {
                if !dirs.iter().any(|x| x.to_string_lossy().to_lowercase() == dir.to_string_lossy().to_lowercase()) {
                    dirs.push(dir.to_path_buf());
                }
            }
        }
        remove_thumb(&d.thumb);
    }
    if !drafts.is_empty() {
        lib.clips.retain(|c| !c.draft || keep.contains(&c.id));
        lib.save();
    }
    let mut n = 0;
    for dir in &dirs {
        use std::os::windows::fs::MetadataExt;
        const REPARSE: u32 = 0x400;
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            // symlink_metadata: a link is judged by itself, never by what it points at.
            let Ok(md) = fs::symlink_metadata(e.path()) else { continue };
            if kept.contains(&e.path().to_string_lossy().to_lowercase()) {
                continue; // a project's draft
            }
            if md.is_file() && md.file_attributes() & REPARSE == 0 && fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
        let _ = fs::remove_dir(dir); // only when empty
    }
    if n > 0 || !drafts.is_empty() {
        util::log(&format!("drafts: {} entries and {n} files removed at start", drafts.len()));
    }
    n
}
