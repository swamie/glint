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
    /// "clip" | "trim" | "edit" | "blur" | "import"
    pub kind: String,
    pub thumb: String,
    /// 3 = mix + desktop + mic, 1 = single track, 0 = silent.
    pub audio_tracks: u32,
    /// Subfolder of the clips folder that holds the file. "" = the clips folder itself.
    pub folder: String,
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

    pub fn remove(&mut self, id: &str) -> Option<ClipMeta> {
        let pos = self.clips.iter().position(|c| c.id == id)?;
        let meta = self.clips.remove(pos);
        self.save();
        Some(meta)
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

/// Grab one frame as the library thumbnail. Returns the jpg path ("" on failure).
pub fn make_thumb(ffmpeg: &Path, video: &Path, id: &str, at: f64) -> String {
    let out = thumbs_dir().join(format!("{id}.jpg"));
    // A clip of a still screen may hold no frame at `at`; fall back to the first frame.
    for pos in [at, 0.0] {
        let _ = util::hidden_cmd(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{pos:.2}"), "-i"])
            .arg(video)
            .args(["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "5"])
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
    let id = util::new_id();
    let p = probe(ffmpeg, file, false).unwrap_or_default();
    ClipMeta {
        thumb: make_thumb(ffmpeg, file, &id, (p.duration * 0.5).min(5.0)),
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
    }
}
