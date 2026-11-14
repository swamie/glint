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
}

pub struct Library {
    pub clips: Vec<ClipMeta>,
}

impl Library {
    fn file() -> PathBuf {
        util::data_dir().join("library.json")
    }

    pub fn load() -> Self {
        let mut clips: Vec<ClipMeta> = fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        clips.retain(|c| Path::new(&c.path).is_file());
        Library { clips }
    }

    pub fn save(&self) {
        if let Ok(t) = serde_json::to_string(&self.clips) {
            let _ = fs::write(Self::file(), t);
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
    let ffprobe = ffprobe_for(ffmpeg)?;
    let mut cmd = util::hidden_cmd(&ffprobe);
    cmd.args(["-v", "error"]);
    if count_frames {
        cmd.arg("-count_packets");
    }
    cmd.args(["-show_entries", "stream=codec_type,width,height,avg_frame_rate,nb_read_packets:format=duration", "-of", "default=nw=1"]);
    cmd.arg(file).stdin(Stdio::null()).stderr(Stdio::null());
    let out = cmd.output().ok()?;
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
    let out = thumbs_dir().join(format!("{id}.jpg"));
    // A clip of a still screen may hold no frame at `at`; fall back to the first frame.
    for pos in [at, 0.0] {
        let _ = util::hidden_cmd(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{pos:.2}"), "-i"])
            .arg(video)
            .args(["-frames:v", "1", "-vf", THUMB_VF, "-q:v", "5"])
            .arg(&out)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if out.is_file() && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false) {
            return out.to_string_lossy().into_owned();
        }
    }
    String::new()
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
