//! Import clips recorded by other clipping software (NVIDIA ShadowPlay, Medal,
//! Outplayed, SteelSeries GG Moments) or from any folder, tagged by game.
//!
//! Contract with the UI (`ui/importer.js`): the `import_sources`,
//! `import_scan` and `import_run` commands in `app.rs`, and an
//! "import-progress" event ({ done, total, title }) while an import runs. The
//! last progress event of a run also carries `skipped` (files that could not be
//! read). "clips-changed" is sent every 25 clips and at the end.
//!
//! Other programs' files and settings are only ever read: an import never moves,
//! renames, changes or deletes an original. With "copy" the file is copied into
//! Glint's clips folder; otherwise the library simply points at it.
//!
//! Where each program keeps its clips (checked on a PC with Medal and the NVIDIA
//! app installed, November 2026):
//!
//! * NVIDIA ShadowPlay / NVIDIA app. Folder: `settings.currentDirectoryV2` in
//!   `%LOCALAPPDATA%\NVIDIA Corporation\NVIDIA Overlay\GallerySettings.json`
//!   (NVIDIA app, seen), else the registry value `DefaultPathW` (REG_BINARY,
//!   UTF-16) under `HKCU\Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS`
//!   (GeForce Experience and the NVIDIA app both write it, seen), else
//!   `<Videos>\NVIDIA` (the NVIDIA app's default, seen). GeForce Experience used
//!   `<Videos>` itself by default; that is not offered, since it would list
//!   every video the user has. Layout (seen): `<root>\<Game>\<Game> 2024.05.01 -
//!   12.34.56.78.DVR.mp4` for Instant Replay, the same without `.DVR` for manual
//!   recordings; `Desktop` and `Base Profile` folders hold non-game captures.
//! * Medal. Folder: the `ClipFolder` entry of the `key_values` table in Medal's
//!   SQLite index `%APPDATA%\Medal\medal-<user>.db` (seen; Medal keeps a copy in
//!   `<ClipFolder>\Databases`), default `<Videos>\Medal` (seen). Layout (seen):
//!   `<root>\Clips\<Game>\MedalTV<GameWithoutSpaces><yyyyMMddHHmmssfff>.mp4`,
//!   desktop captures in `Clips\Screen Recording`, trims in
//!   `<root>\Edits\MedalTV<Game><stamp>-trim-<ms>.mp4` (no game folder), plus
//!   Thumbnails, Reels (jpg), Temp, Databases and Video-Editor, which are skipped.
//!   The index stores each clip's path and a `categoryName` (the game) in a
//!   binary record; it is read without SQLite, by pattern, and never written.
//! * Outplayed (Overwolf). Folder: chosen in Outplayed's storage settings (no
//!   readable settings file is known); default `<Videos>\Overwolf\Outplayed`
//!   (from Overwolf developer logs and support threads; not installed here).
//!   Layout (from the same sources, not seen): `<root>\<Game>\<Game>_<M-D-YYYY>_
//!   <H-M-S-ms>\<Game> <MM-DD-YYYY>_<HH-MM-SS-ms>.mp4` (one session folder per
//!   match), with split files in `tmp`, which is skipped.
//! * SteelSeries GG Moments. Folder: Settings > Moments > Storage location; GG
//!   keeps its settings under `%PROGRAMDATA%\SteelSeries\GG`, whose JSON files are
//!   searched for a folder value (assumed: GG is not installed here); default
//!   `<Videos>\SteelSeries Moments` (seen: the folder was left behind by GG, with
//!   `Exported Clips`, `Shared Clips` and a hidden `thumbs`). SteelSeries says
//!   clips are named after the game; the per-game folder layout is assumed.
//!
//! The game of each video comes from, in order: its folders (deepest first,
//! skipping containers such as "Clips" and dated session folders), its file name
//! (a game name followed by a date), Medal's index, else "Imported".
use crate::core::Core;
use crate::{library, util};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

/// A place clips may be found: one of the known programs, with the folder it saves to on this PC.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Source {
    /// "shadowplay" | "medal" | "outplayed" | "steelseries"
    pub id: String,
    /// Name to show, e.g. "NVIDIA ShadowPlay".
    pub name: String,
    /// Its clips folder on this PC ("" if not found).
    pub path: String,
    /// The folder exists.
    pub found: bool,
}

/// One video found by a scan. The UI sends the chosen ones back to `import_run`.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Found {
    pub path: String,
    /// File name without extension.
    pub title: String,
    /// Game it belongs to, worked out from the folder layout or the file name ("Imported" if unknown).
    pub game: String,
    pub size: u64,
    /// Unix milliseconds the file was last modified (used as the clip's date).
    pub modified: u64,
    /// Already in Glint's library.
    pub known: bool,
}

const UNKNOWN: &str = "Imported";
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "mov"];
/// Folders below the scanned one that are still searched.
const MAX_DEPTH: usize = 6;
/// A scan stops collecting after this many videos.
const MAX_FILES: usize = 50_000;
const WORKERS: usize = 4;

/// Folder names never searched: thumbnails, temporary files, databases and editor projects these programs keep.
const SKIP_DIRS: &[&str] = &[
    "thumbnails", "thumbs", "thumb", "temp", "tmp", "cache", "databases", "video-editor", "reels", "logs",
    "$recycle.bin", "system volume information", "node_modules",
];

/// Folder names that hold clips but do not name a game.
const CONTAINERS: &[&str] = &[
    "clips", "clip", "videos", "video", "my videos", "recordings", "recording", "captures", "capture", "replays", "replay",
    "instant replay", "instant replays", "highlights", "exported clips", "shared clips", "edits", "edited", "montages",
    "medal", "nvidia", "shadowplay", "geforce experience", "outplayed", "overwolf", "steelseries", "steelseries moments",
    "moments", "sessions", "session", "full sessions", "full session", "my clips", "imported", "import", "new folder",
    "gameplay", "game clips", "games", "glint", "manual recordings", "screenshots", "insights capture",
];

/// Names these programs give to captures of the desktop or a non-game window.
const DESKTOP_NAMES: &[&str] = &["desktop", "screen recording", "screen capture", "screen", "base profile", "windows", "explorer", "windows explorer", "desktop recording"];

// ---------------------------------------------------------------------------
// Where the programs keep their clips

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The user's Videos folder, wherever it was moved to.
fn videos_dir() -> PathBuf {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    if let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders") {
        if let Ok(v) = k.get_value::<String, _>("My Video") {
            if Path::new(&v).is_dir() {
                return PathBuf::from(v);
            }
        }
    }
    env_dir("USERPROFILE").unwrap_or_default().join("Videos")
}

/// The first folder that exists, else the first candidate (not found).
fn pick(cands: Vec<PathBuf>) -> (String, bool) {
    let cands: Vec<PathBuf> = cands.into_iter().filter(|c| !c.as_os_str().is_empty()).collect();
    match cands.iter().find(|c| c.is_dir()) {
        Some(p) => (p.to_string_lossy().into_owned(), true),
        None => (cands.first().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), false),
    }
}

fn nvidia_dirs(videos: &Path) -> Vec<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let mut out = Vec::new();
    if let Some(local) = env_dir("LOCALAPPDATA") {
        let f = local.join(r"NVIDIA Corporation\NVIDIA Overlay\GallerySettings.json");
        if let Some(v) = fs::read_to_string(f).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            if let Some(dir) = v.pointer("/settings/currentDirectoryV2").and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()) {
                out.push(PathBuf::from(dir.trim()));
            }
        }
    }
    if let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\NVIDIA Corporation\Global\ShadowPlay\NVSPCAPS") {
        if let Ok(raw) = k.get_raw_value("DefaultPathW") {
            let wide: Vec<u16> = raw.bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            let s = String::from_utf16_lossy(&wide);
            if !s.trim().is_empty() {
                out.push(PathBuf::from(s.trim()));
            }
        }
    }
    out.push(videos.join("NVIDIA"));
    out
}

/// Read a file another program may hold open, without ever asking for write access.
fn read_shared(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: Medal keeps its index open.
    let f = fs::OpenOptions::new().read(true).share_mode(7).open(path).ok()?;
    if f.metadata().ok()?.len() > limit {
        return None;
    }
    let mut buf = Vec::new();
    f.take(limit).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Medal's index files: `%APPDATA%\Medal\medal-*.db` (and their write-ahead logs), plus any in `extra` folders.
fn medal_db_files(extra: &[PathBuf], appdata: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = if appdata { env_dir("APPDATA").map(|a| vec![a.join("Medal")]).unwrap_or_default() } else { Vec::new() };
    dirs.extend(extra.iter().cloned());
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.starts_with("medal") && (n.ends_with(".db") || n.ends_with(".db-wal")) && e.path().is_file() {
                out.push(e.path());
            }
        }
    }
    out
}

/// A string as Medal's record format stores it at `at`: a tag byte whose low
/// nibble is 7 (text) or 8 (JSON text) and whose high nibble is the length, or
/// 0xC7 / 0xC8 followed by a one-byte length.
fn tagged_str(b: &[u8], at: usize) -> Option<(String, bool)> {
    let tag = *b.get(at)?;
    let kind = tag & 0x0f;
    if kind != 7 && kind != 8 {
        return None;
    }
    let (len, start) = if tag >> 4 == 0xc { (*b.get(at + 1)? as usize, at + 2) } else if tag >> 4 < 0xc { ((tag >> 4) as usize, at + 1) } else { return None };
    if len == 0 {
        return None;
    }
    let s = std::str::from_utf8(b.get(start..start + len)?).ok()?;
    Some((s.to_string(), kind == 8))
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return out;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i] == first && &hay[i..i + needle.len()] == needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// Folders found in Medal's `ClipFolder` setting.
fn medal_clip_folders(b: &[u8]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for at in find_all(b, b"ClipFolder") {
        let v = at + "ClipFolder".len();
        if b[v..].starts_with(b"VolumeId") {
            continue;
        }
        let Some((s, json)) = tagged_str(b, v) else { continue };
        let s = if json { serde_json::from_str::<String>(&format!("\"{s}\"")).unwrap_or_else(|_| s.replace("\\\\", "\\")) } else { s };
        let s = s.trim().trim_matches('"').to_string();
        if s.len() > 2 && s.as_bytes()[1] == b':' {
            out.push(PathBuf::from(s));
        }
    }
    out
}

/// Medal's index, read by pattern: video file name (lower case) -> game
/// (`categoryName`), taken from the first category that follows the clip's path
/// in the same record.
fn medal_index(b: &[u8]) -> HashMap<String, String> {
    // Every "X:\....mp4" path, by where it ends, with its file name.
    let mut paths: Vec<(usize, String)> = Vec::new();
    for at in find_all(b, b".mp4") {
        let end = at + 4;
        let mut start = at;
        while start > 0 && at - start < 400 {
            let c = b[start - 1];
            if c < 0x20 || c == b'"' {
                break;
            }
            start -= 1;
        }
        let seg = &b[start..end];
        // The path proper starts at the last "X:\" in the segment.
        let Some(drive) = (0..seg.len().saturating_sub(2)).rev().find(|&i| seg[i].is_ascii_alphabetic() && seg[i + 1] == b':' && seg[i + 2] == b'\\') else { continue };
        let Ok(path) = std::str::from_utf8(&seg[drive..]) else { continue };
        let name = path.rsplit('\\').next().unwrap_or_default().to_lowercase();
        if name.len() > 4 {
            paths.push((end, name));
        }
    }
    let mut map = HashMap::new();
    for at in find_all(b, b"categoryName") {
        let Some((game, _)) = tagged_str(b, at + "categoryName".len()) else { continue };
        let game = game.trim().to_string();
        if game.is_empty() || game.len() > 80 {
            continue;
        }
        let i = paths.partition_point(|(end, _)| *end <= at);
        if i == 0 {
            continue;
        }
        let (end, name) = &paths[i - 1];
        if at - end < 32 * 1024 {
            map.entry(name.clone()).or_insert(game);
        }
    }
    map
}

fn medal_dirs(videos: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for f in medal_db_files(&[], true) {
        if let Some(b) = read_shared(&f, 512 * 1024 * 1024) {
            for d in medal_clip_folders(&b) {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
    }
    out.push(videos.join("Medal"));
    out
}

fn outplayed_dirs(videos: &Path) -> Vec<PathBuf> {
    vec![videos.join("Overwolf").join("Outplayed")]
}

/// Folder values in SteelSeries GG's JSON settings whose key mentions Moments
/// storage. Assumed format (GG is not installed on the PC this was written on).
fn steelseries_dirs(videos: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(pd) = env_dir("PROGRAMDATA") {
        let mut stack = vec![(pd.join("SteelSeries").join("GG"), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if depth < 4 {
                        stack.push((p, depth + 1));
                    }
                    continue;
                }
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) || e.metadata().map(|m| m.len() > 2_000_000).unwrap_or(true) {
                    continue;
                }
                let Some(v) = read_shared(&p, 2_000_000).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { continue };
                collect_folder_values(&v, "", &mut out);
            }
        }
    }
    out.retain(|p| p.is_dir());
    out.push(videos.join("SteelSeries Moments"));
    out
}

fn collect_folder_values(v: &serde_json::Value, key: &str, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::Object(m) => m.iter().for_each(|(k, x)| collect_folder_values(x, k, out)),
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect_folder_values(x, key, out)),
        serde_json::Value::String(s) => {
            let k = key.to_lowercase();
            if (k.contains("path") || k.contains("folder") || k.contains("dir") || k.contains("location")) && (k.contains("clip") || k.contains("moment") || k.contains("storage") || k.contains("capture")) && s.len() > 3 && s.as_bytes()[1] == b':' {
                out.push(PathBuf::from(s));
            }
        }
        _ => {}
    }
}

/// The known programs and where each keeps its clips on this PC.
pub fn sources() -> Vec<Source> {
    let videos = videos_dir();
    let mk = |id: &str, name: &str, cands: Vec<PathBuf>| {
        let (path, found) = pick(cands);
        Source { id: id.into(), name: name.into(), path, found }
    };
    vec![
        mk("shadowplay", "NVIDIA ShadowPlay", nvidia_dirs(&videos)),
        mk("medal", "Medal", medal_dirs(&videos)),
        mk("outplayed", "Outplayed", outplayed_dirs(&videos)),
        mk("steelseries", "SteelSeries GG Moments", steelseries_dirs(&videos)),
    ]
}

// ---------------------------------------------------------------------------
// Game names

fn is_sep(c: char) -> bool {
    matches!(c, ' ' | '.' | '_' | '-' | ',' | '(' | ')' | '[' | ']')
}

/// Whether trailing numeric tokens read as a date (and maybe a time).
fn date_like(toks: &[&str]) -> bool {
    let year = |s: &str| s.len() == 4 && (s.starts_with("19") || s.starts_with("20"));
    let Some(first) = toks.first() else { return false };
    (year(first) && toks.len() >= 3)
        || (first.len() >= 8 && (first.starts_with("19") || first.starts_with("20")))
        || (first.len() <= 2 && toks.len() >= 3 && year(toks[2]))
}

/// Remove a trailing ".DVR" and Medal's "-trim-<ms>" from a file stem.
fn strip_marks(stem: &str) -> String {
    let mut s = stem.trim().to_string();
    loop {
        let lower = s.to_lowercase();
        if lower.ends_with(".dvr") {
            s.truncate(s.len() - 4);
        } else if let Some(at) = lower.rfind("-trim-").filter(|&at| s[at + 6..].chars().all(|c| c.is_ascii_digit()) && s.len() > at + 6) {
            s.truncate(at);
        } else {
            break;
        }
        s = s.trim().to_string();
    }
    s
}

/// A name with any trailing date and time removed, and ".DVR" / "-trim-" marks gone.
/// Returns the cleaned name and whether a date was removed.
pub fn clean_name(raw: &str) -> (String, bool) {
    let s = strip_marks(raw);
    // Tokens with their byte offsets.
    let mut toks: Vec<(usize, &str)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.char_indices() {
        if is_sep(c) {
            if let Some(st) = start.take() {
                toks.push((st, &s[st..i]));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        toks.push((st, &s[st..]));
    }
    let mut run = toks.len();
    while run > 0 && toks[run - 1].1.chars().all(|c| c.is_ascii_digit()) {
        run -= 1;
    }
    let words: Vec<&str> = toks.iter().map(|(_, w)| *w).collect();
    let mut cut = None;
    for at in run..toks.len() {
        if date_like(&words[at..]) {
            cut = Some(toks[at].0);
            break;
        }
    }
    let out = match cut {
        Some(at) => s[..at].trim_end_matches(|c: char| is_sep(c) || c.is_whitespace()).trim().to_string(),
        None => s.trim().to_string(),
    };
    (out, cut.is_some())
}

fn is_container(name: &str) -> bool {
    CONTAINERS.contains(&name.trim().to_lowercase().as_str())
}

/// "Desktop" for the programs' names for desktop captures, else the name.
fn desktop_map(name: &str) -> String {
    // One name per game ("Overwatch 2" and "Overwatch" are the same), and names that are no game at all become Desktop.
    if DESKTOP_NAMES.contains(&name.trim().to_lowercase().as_str()) { crate::games::DESKTOP.to_string() } else { crate::games::canonical(&name.trim().chars().take(60).collect::<String>()) }
}

/// The game a folder name stands for, if any.
fn dir_game(dir: &str) -> Option<String> {
    let (name, _) = clean_name(dir);
    if name.is_empty() || is_container(&name) || name.chars().all(|c| c.is_ascii_digit() || is_sep(c)) {
        return None;
    }
    Some(desktop_map(&name))
}

fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Publisher prefixes some programs keep in a game's name and others drop
/// (Medal files "Rainbow Six Siege" under "Tom Clancys Rainbow Six Siege"), compacted.
const NAME_PREFIXES: &[&str] = &["tomclancys", "sidmeiers"];

/// One key per game however it is spelled: case, spaces, punctuation, "™" and
/// a publisher prefix ignored. "Tom Clancy's Rainbow Six® Siege" and
/// "Rainbow Six Siege" share "rainbowsixsiege".
fn game_key(s: &str) -> String {
    let c = compact(s);
    for p in NAME_PREFIXES {
        if let Some(rest) = c.strip_prefix(p) {
            if rest.len() >= 3 {
                return rest.to_string();
            }
        }
    }
    c
}

/// What a scan knows beyond the files themselves.
#[derive(Default)]
struct Ctx {
    /// Medal index: file name (lower case) -> game.
    index: HashMap<String, String>,
    /// Game names by their compact spelling ("riskofrain2" -> "Risk of Rain 2"), longest first.
    compact: Vec<(String, String)>,
}

impl Ctx {
    fn add_name(&mut self, name: &str) {
        let k = compact(name);
        if k.len() >= 2 && !self.compact.iter().any(|(c, _)| *c == k) {
            self.compact.push((k, name.to_string()));
        }
    }
    fn finish(&mut self) {
        self.compact.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }
}

/// The game in a Medal file name ("MedalTVRiskofRain220261023215833072"), matched against known names.
fn medal_known(rest: &str, ctx: &Ctx) -> Option<String> {
    // As written, then without a publisher prefix ("TomClancysRainbowSixSiege2026..." also matches "Rainbow Six Siege").
    let mut tries = vec![compact(rest)];
    let k = game_key(rest);
    if k != tries[0] {
        tries.push(k);
    }
    tries.iter().find_map(|c| ctx.compact.iter().find(|(k, _)| c.starts_with(k.as_str()) && c[k.len()..].len() >= 8 && c[k.len()..].chars().all(|x| x.is_ascii_digit())).map(|(_, n)| desktop_map(n)))
}

/// The game in a Medal file name without help: the part before the 17-digit time stamp.
fn medal_naive(rest: &str) -> Option<String> {
    let digits = rest.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits < 14 {
        return None;
    }
    let keep = rest.len() - digits.min(17);
    let name = rest[..keep].trim();
    (!name.is_empty()).then(|| desktop_map(name))
}

/// Game for a video at `rel_dirs` (folders below the scanned one) named `stem`.
fn game_for(rel_dirs: &[String], stem: &str, ctx: &Ctx) -> String {
    for d in rel_dirs.iter().rev() {
        if let Some(g) = dir_game(d) {
            return g;
        }
    }
    let base = strip_marks(stem);
    let medal = base.strip_prefix("MedalTV").map(str::to_string);
    if let Some(rest) = &medal {
        if let Some(g) = medal_known(rest, ctx) {
            return g;
        }
    } else {
        let (name, dated) = clean_name(&base);
        if dated && !name.is_empty() && !is_container(&name) {
            return desktop_map(&name);
        }
    }
    for ext in VIDEO_EXTS {
        if let Some(g) = ctx.index.get(&format!("{}.{ext}", stem.to_lowercase())) {
            return desktop_map(g);
        }
    }
    if let Some(g) = medal.as_deref().and_then(medal_naive) {
        return g;
    }
    UNKNOWN.to_string()
}

// ---------------------------------------------------------------------------
// Scan

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

fn mtime_ms(md: &fs::Metadata) -> u64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

struct Walk {
    skip: Vec<String>,
    files: Vec<(PathBuf, Vec<String>, fs::Metadata)>,
    /// "Databases" folders seen (Medal keeps a copy of its index there).
    db_dirs: Vec<PathBuf>,
}

fn walk(w: &mut Walk, dir: &Path, rel: &mut Vec<String>) {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const REPARSE: u32 = 0x400;
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<fs::DirEntry> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if w.files.len() >= MAX_FILES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(md) = e.metadata() else { continue };
        let attrs = md.file_attributes();
        if name.starts_with('.') || name.starts_with('$') || name.starts_with('~') || attrs & (HIDDEN | SYSTEM) != 0 {
            continue;
        }
        let p = e.path();
        if md.is_dir() {
            if attrs & REPARSE != 0 {
                continue;
            }
            let lower = name.to_lowercase();
            if lower == "databases" {
                w.db_dirs.push(p.clone());
            }
            if SKIP_DIRS.contains(&lower.as_str()) || w.skip.contains(&norm(&p)) || rel.len() >= MAX_DEPTH {
                continue;
            }
            rel.push(name);
            walk(w, &p, rel);
            rel.pop();
        } else if md.is_file() && md.len() > 0 {
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            if VIDEO_EXTS.contains(&ext.as_str()) {
                w.files.push((p, rel.clone(), md));
            }
        }
    }
}

/// One clip already imported: where it came from, and its library id.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Record {
    source: String,
    id: String,
}

static RECORDS: Mutex<()> = Mutex::new(());

fn records_file() -> PathBuf {
    util::data_dir().join("imports.json")
}

fn load_records() -> Vec<Record> {
    fs::read_to_string(records_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Paths (lower case) already in the library, directly or as the source of a copy that is.
fn known_paths(core: &Core) -> HashSet<String> {
    let lib = core.lib.lock().unwrap();
    let ids: HashSet<&str> = lib.clips.iter().map(|c| c.id.as_str()).collect();
    let mut out: HashSet<String> = lib.clips.iter().map(|c| c.path.to_lowercase()).collect();
    let _g = RECORDS.lock().unwrap();
    for r in load_records() {
        if ids.contains(r.id.as_str()) {
            out.insert(r.source.to_lowercase());
        }
    }
    out
}

/// List the videos under `path` (a source's folder or any folder the user picked), with a game for each.
pub fn scan(core: &Arc<Core>, path: &str) -> Result<Vec<Found>, String> {
    scan_with(core, path, true)
}

/// `real` = also consult the programs' own folders and Medal's index under
/// %APPDATA% (off in the self-test, which must only see its scratch trees).
fn scan_with(core: &Arc<Core>, path: &str, real: bool) -> Result<Vec<Found>, String> {
    let root = PathBuf::from(path.trim());
    if path.trim().is_empty() || !root.is_dir() {
        return Err("That folder does not exist".into());
    }
    let clips = PathBuf::from(core.settings().clips_dir);
    let (r, c) = (norm(&root), norm(&clips));
    if !c.is_empty() && (r == c || r.starts_with(&format!("{c}\\"))) {
        return Err("This is Glint's own clips folder: its clips are already in the library".into());
    }
    let data = util::data_dir();
    let mut w = Walk { skip: vec![c, norm(&data.join("tmp")), norm(&data.join("thumbs"))], files: Vec::new(), db_dirs: Vec::new() };
    walk(&mut w, &root, &mut Vec::new());
    let medal_roots: Vec<PathBuf> = if real { sources().into_iter().filter(|s| s.id == "medal" && s.found).map(|s| PathBuf::from(s.path)).collect() } else { Vec::new() };

    let mut ctx = Ctx::default();
    // Game names from folders, and from Medal's game folders ("<root>\Clips\<Game>").
    for (_, rel, _) in &w.files {
        if let Some(g) = rel.iter().rev().find_map(|d| dir_game(d)) {
            ctx.add_name(&g);
        }
    }
    for m in &medal_roots {
        if let Ok(rd) = fs::read_dir(m.join("Clips")) {
            for e in rd.flatten().filter(|e| e.path().is_dir()) {
                ctx.add_name(&e.file_name().to_string_lossy());
            }
        }
    }
    // Medal's index, only when there are Medal files to name.
    if w.files.iter().any(|(p, _, _)| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("MedalTV"))) {
        let mut extra = w.db_dirs.clone();
        extra.extend(medal_roots.iter().map(|m| m.join("Databases")));
        for f in medal_db_files(&extra, real) {
            if let Some(b) = read_shared(&f, 512 * 1024 * 1024) {
                for (k, v) in medal_index(&b) {
                    ctx.index.entry(k).or_insert(v);
                }
            }
        }
        let names: Vec<String> = ctx.index.values().cloned().collect();
        names.iter().for_each(|n| ctx.add_name(n));
    }
    ctx.finish();
    // One spelling per game (`game_key`): Medal's own name for it from its index
    // (Medal's folders drop characters and keep publisher prefixes: "osu!" is
    // filed under "osu", "Rainbow Six Siege" under "Tom Clancys Rainbow Six
    // Siege"), else the one Glint already uses, a name without a prefix first.
    let mut proper: HashMap<String, String> = HashMap::new();
    let mut idx_names: Vec<&String> = ctx.index.values().collect();
    idx_names.sort();
    for g in idx_names {
        proper.entry(game_key(g)).or_insert_with(|| desktop_map(g));
    }
    {
        let games = core.games.lock().unwrap();
        let (plain, prefixed): (Vec<_>, Vec<_>) = games.list.iter().partition(|g| game_key(&g.name) == compact(&g.name));
        for g in plain.into_iter().chain(prefixed) {
            proper.entry(game_key(&g.name)).or_insert_with(|| g.name.clone());
        }
    }
    let spell = |g: String| -> String {
        if g == UNKNOWN || g == crate::games::DESKTOP {
            return g;
        }
        proper.get(&game_key(&g)).cloned().unwrap_or(g)
    };

    let known = known_paths(core);
    let mut out: Vec<Found> = w
        .files
        .iter()
        .map(|(p, rel, md)| {
            let title = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let path = p.to_string_lossy().into_owned();
            Found { game: spell(game_for(rel, &title, &ctx)), known: known.contains(&path.to_lowercase()), path, title, size: md.len(), modified: mtime_ms(md) }
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Import

/// The library folder a file is in: its directory's name if that sits directly inside the clips folder, else "".
fn folder_of(clips_dir: &Path, file: &Path) -> String {
    let Some(parent) = file.parent() else { return String::new() };
    match (parent.parent(), parent.file_name()) {
        (Some(grand), Some(name)) if norm(grand) == norm(clips_dir) => name.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

/// Which library folder an import puts its clips in ("Put the imported clips in").
///
/// A library folder is a subfolder of the clips folder. A copy is written into
/// it. A clip imported in place stays where it is (its file belongs to the other
/// program and is never moved, changed or deleted): the folder lists it by name
/// (`ClipMeta.folder`), and `Core::rescan`, `move_clips`, `rename_folder` and
/// `delete_folder` keep it that way without touching the file.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// A folder per game, named as Glint names game folders (`util::game_folder`).
    Games,
    /// Every clip of this import in one folder.
    One(String),
    /// The main folder.
    Main,
}

/// The UI's choice: `place` "games" | "one" | "none" with `folder` the name for
/// "one". Without a choice (an older caller): game folders when that setting is
/// on, else the main folder, as before.
pub fn place_from(place: Option<&str>, folder: Option<&str>, game_folders: bool) -> Result<Place, String> {
    match place.map(str::trim).unwrap_or("") {
        "games" | "game" => Ok(Place::Games),
        "none" | "main" => Ok(Place::Main),
        "one" => {
            let name = folder.unwrap_or("").trim();
            if name.trim_matches('.').trim().is_empty() {
                return Err("Type a folder name".into());
            }
            Ok(Place::One(util::sanitize_filename(name)))
        }
        "" => Ok(if game_folders { Place::Games } else { Place::Main }),
        other => Err(format!("Unknown place \"{other}\"")),
    }
}

struct Job<'a> {
    core: &'a Arc<Core>,
    ffmpeg: &'a Path,
    root: &'a Path,
    place: Place,
    copy: bool,
    /// Held while a copy's file name is chosen and claimed.
    naming: &'a Mutex<()>,
    /// The library folders that exist (spelled as on disk), grown as folders are made.
    folders: &'a Mutex<Vec<String>>,
}

impl Job<'_> {
    /// The library folder for a clip of `game` ("" = the main folder), made if
    /// needed, spelled as an existing folder of the same name is (Windows names
    /// ignore case; the library compares names exactly).
    fn folder_for(&self, game: &str) -> Result<String, String> {
        let want = match &self.place {
            Place::Games => util::game_folder(game),
            Place::One(n) => n.clone(),
            Place::Main => return Ok(String::new()),
        };
        let mut known = self.folders.lock().unwrap();
        if let Some(f) = known.iter().find(|f| f.eq_ignore_ascii_case(&want)) {
            return Ok(f.clone());
        }
        fs::create_dir_all(self.root.join(&want)).map_err(|e| format!("Could not create the folder \"{want}\": {e}"))?;
        known.push(want.clone());
        Ok(want)
    }
}

fn import_one(j: &Job, it: &Found) -> Result<library::ClipMeta, String> {
    let src = PathBuf::from(&it.path);
    let md = fs::metadata(&src).map_err(|e| e.to_string())?;
    if !md.is_file() {
        return Err("not a file".into());
    }
    let modified = mtime_ms(&md);
    let game: String = if it.game.trim().is_empty() { UNKNOWN.to_string() } else { it.game.trim().chars().take(60).collect() };
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Clip".into());
    let title = if it.title.trim().is_empty() { stem.clone() } else { it.title.clone() };
    let mut copied: Option<PathBuf> = None;
    let lib_folder = j.folder_for(&game)?;
    let file = if j.copy {
        let dir = if lib_folder.is_empty() { j.root.to_path_buf() } else { j.root.join(&lib_folder) };
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
        let dest = {
            let _g = j.naming.lock().unwrap();
            let p = util::unique_path(&dir, &stem, &ext);
            // Claim the name now, so no other worker (or file) can take it before the copy lands.
            fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(|e| e.to_string())?;
            p
        };
        if let Err(e) = fs::copy(&src, &dest) {
            let _ = fs::remove_file(&dest);
            return Err(e.to_string());
        }
        copied = Some(dest.clone());
        dest
    } else {
        src.clone()
    };
    let mut meta = library::register(j.ffmpeg, &file, &title, &game, "import");
    if meta.duration <= 0.0 || meta.width == 0 {
        if !meta.thumb.is_empty() {
            let _ = fs::remove_file(&meta.thumb);
        }
        if let Some(c) = copied {
            // Only ever our own copy: the original is not touched.
            let _ = fs::remove_file(c);
        }
        return Err("not a readable video".into());
    }
    meta.created = if modified > 0 { modified } else { meta.created };
    // A copy is in the folder it was written to. A file left in place is filed
    // by name only (it was filed under "" before 0.1.16, whatever the setting:
    // the bug where Medal clips stayed in the main folder).
    meta.folder = if crate::core::in_clips_dir(j.root, &file.to_string_lossy()) { folder_of(j.root, &file) } else { lib_folder };
    j.core.lib.lock().unwrap().add(meta.clone());
    j.core.games.lock().unwrap().note(&game, "");
    Ok(meta)
}

/// Add `items` to the library, tagged with their games. With `copy`, the files
/// are copied into Glint's clips folder first; otherwise they stay where they
/// are and are only indexed. `place` / `folder` pick the library folder (see
/// `place_from`). The originals are never moved, changed or deleted. Returns
/// how many were added.
pub fn run(core: &Arc<Core>, items: Vec<Found>, copy: bool, place: Option<&str>, folder: Option<&str>) -> Result<usize, String> {
    let place = place_from(place, folder, core.settings().game_folders)?;
    run_placed(core, items, copy, place).map(|(added, _)| added)
}

/// `run` as before 0.1.16 (place from the game folders setting), also returning how many files were skipped.
fn run_counted(core: &Arc<Core>, items: Vec<Found>, copy: bool) -> Result<(usize, usize), String> {
    let place = place_from(None, None, core.settings().game_folders)?;
    run_placed(core, items, copy, place)
}

/// `run`, also returning how many files were skipped as unreadable.
fn run_placed(core: &Arc<Core>, items: Vec<Found>, copy: bool, place: Place) -> Result<(usize, usize), String> {
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg was not found. Set its path in Settings.")?;
    let root = PathBuf::from(&s.clips_dir);
    if copy || place != Place::Main {
        fs::create_dir_all(&root).map_err(|e| format!("Could not create the clips folder: {e}"))?;
    }
    let folders = Mutex::new(core.folders());
    let known = known_paths(core);
    let mut seen = HashSet::new();
    let todo: Vec<Found> = items.into_iter().filter(|i| !i.path.trim().is_empty() && seen.insert(i.path.to_lowercase()) && !known.contains(&i.path.to_lowercase())).collect();
    let total = todo.len();
    let (next, done, added, skipped) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let naming = Mutex::new(());
    let new_records: Mutex<Vec<Record>> = Mutex::new(Vec::new());
    let one = if let Place::One(n) = &place { Some(n.clone()) } else { None };
    let job = Job { core, ffmpeg: &ffmpeg, root: &root, place, copy, naming: &naming, folders: &folders };
    // "One folder": made (or matched to an existing one) before the workers start; a bad name fails here.
    if let Some(n) = one {
        job.folder_for(&n)?;
    }
    let workers = WORKERS.min(total).min(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).max(1));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(it) = todo.get(i) else { break };
                match import_one(&job, it) {
                    Ok(meta) => {
                        new_records.lock().unwrap().push(Record { source: it.path.clone(), id: meta.id });
                        if (added.fetch_add(1, Ordering::SeqCst) + 1) % 25 == 0 {
                            core.emit("clips-changed", ());
                        }
                    }
                    Err(e) => {
                        skipped.fetch_add(1, Ordering::SeqCst);
                        util::log(&format!("import: skipped {}: {e}", it.path));
                    }
                }
                let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                if d < total {
                    core.emit("import-progress", serde_json::json!({ "done": d, "total": total, "title": it.title }));
                }
            });
        }
    });
    let (added, skipped) = (added.into_inner(), skipped.into_inner());
    let recs = new_records.into_inner().unwrap();
    if !recs.is_empty() {
        let _g = RECORDS.lock().unwrap();
        let mut all = load_records();
        all.extend(recs);
        if let Ok(text) = serde_json::to_string(&all) {
            let _ = fs::write(records_file(), text);
        }
    }
    core.emit("import-progress", serde_json::json!({ "done": total, "total": total, "title": "", "skipped": skipped }));
    core.emit("clips-changed", ());
    core.emit("games-changed", ());
    crate::icons::fill_in_background(core.clone()); // icons for the imported games, found on this PC (B)
    util::log(&format!("import: {added} added, {skipped} skipped ({})", if copy { "copied" } else { "in place" }));
    Ok((added, skipped))
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    lines: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let d = detail.as_ref();
        self.lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if ok || d.is_empty() { String::new() } else { format!(" -- {d}") }));
    }
}

fn make_video(ffmpeg: &Path, out: &Path) -> bool {
    for vc in [&["-c:v", "libx264", "-pix_fmt", "yuv420p"][..], &["-c:v", "mpeg4"][..]] {
        let ok = util::hidden_cmd(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=160x90:rate=10", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "1"])
            .args(vc)
            .args(["-c:a", "aac", "-shortest"])
            .arg(out)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false) {
            return true;
        }
    }
    false
}

fn set_mtime(p: &Path, secs: u64) {
    if let Ok(f) = fs::OpenOptions::new().write(true).open(p) {
        let _ = f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs));
    }
}

/// 0.1.16 checks: "Put the imported clips in", clips imported in place inside
/// library folders (never moved), and one game per spelling (Medal's folder
/// "Tom Clancys Rainbow Six Siege" and its index's "Rainbow Six Siege").
fn placement_tests(r: &mut Report, core: &Arc<Core>, base: &Path, clips: &Path, put: &dyn Fn(&str, u64) -> PathBuf) {
    // --- the choice
    r.check("place: no choice follows the game folders setting", place_from(None, None, true) == Ok(Place::Games) && place_from(None, None, false) == Ok(Place::Main), "");
    r.check("place: games / none", place_from(Some("games"), None, false) == Ok(Place::Games) && place_from(Some("none"), Some("x"), true) == Ok(Place::Main), "");
    r.check("place: one folder needs a name", place_from(Some("one"), Some("  "), false).is_err() && place_from(Some("one"), None, false).is_err() && place_from(Some("one"), Some(".."), false).is_err(), "");
    let odd = place_from(Some("one"), Some(" Me:dal? "), false);
    r.check("place: one folder's name is made safe", odd == Ok(Place::One(util::sanitize_filename("Me:dal?"))), format!("{odd:?}"));
    r.check("place: an unknown choice is an error", place_from(Some("elsewhere"), None, false).is_err(), "");

    // --- spellings
    r.check("game key: publisher prefix, case, punctuation and (R) ignored", game_key("Tom Clancy's Rainbow Six\u{ae} Siege") == game_key("Rainbow Six Siege") && game_key("Tom Clancys Rainbow Six Siege") == "rainbowsixsiege", game_key("Tom Clancy's Rainbow Six\u{ae} Siege"));
    r.check("game key: different games stay different", game_key("Rainbow Six Siege") != game_key("Rainbow Six Extraction") && game_key("Tom Clancys") == "tomclancys", "");

    // --- the reported case: a Medal tree where most clips sit in Medal's game
    // folder (Medal's spelling) and some elsewhere, named by Medal's index.
    let t1 = 1_710_000_000u64;
    let in_folder = put(r"src3\Medal\Clips\Tom Clancys Rainbow Six Siege\MedalTVTomClancysRainbowSixSiege20261101101010101.mp4", t1 + 9);
    let elsewhere = put(r"src3\Medal\Clips\MedalTVRainbowSixSiege20261102101010101.mp4", t1 + 8);
    let mut db: Vec<u8> = b"SQLite format 3\0junk\x00\x05".to_vec();
    db.extend_from_slice(b"C:\\Users\\x\\Videos\\Medal\\Clips\\MedalTVRainbowSixSiege20261102101010101.mp4\x00\xc7\x0ccategoryName\xc7\x11Rainbow Six Siege\x47icon\x00");
    let _ = fs::create_dir_all(base.join(r"src3\Medal\Databases"));
    let _ = fs::write(base.join(r"src3\Medal\Databases\medal-1.db"), &db);
    let loose_a = put(r"src3\Loose\Valorant 2024.06.01 - 10.00.00.00.mp4", t1 + 7);
    let loose_b = put(r"src3\Loose\Valorant 2024.06.02 - 10.00.00.00.mp4", t1 + 6);
    let copy_a = put(r"src3\Copies\Apex Legends 2024.06.03 - 10.00.00.00.mp4", t1 + 5);
    let copy_b = put(r"src3\Copies\Apex Legends 2024.06.04 - 10.00.00.00.mp4", t1 + 4);
    let main_a = put(r"src3\Main\Deadlock 2024.06.05 - 10.00.00.00.mp4", t1 + 3);
    let found = match scan_with(core, &base.join("src3").to_string_lossy(), false) {
        Ok(f) => f,
        Err(e) => return r.check("scan the placement tree", false, e),
    };
    // Medal's index names it "Rainbow Six Siege"; Glint files it under games::canonical of that.
    let want = desktop_map("Rainbow Six Siege");
    let wf = util::game_folder(&want);
    let item = |p: &Path| found.iter().find(|f| f.path.eq_ignore_ascii_case(&p.to_string_lossy())).cloned();
    let g = |p: &Path| item(p).map(|f| f.game).unwrap_or_else(|| "(not found)".into());
    r.check("spelling: Medal's folder name and its index's name are one game", g(&in_folder) == g(&elsewhere) && g(&elsewhere) == want, format!("\"{}\" / \"{}\"", g(&in_folder), g(&elsewhere)));
    let items = |ps: &[&PathBuf]| -> Vec<Found> { ps.iter().filter_map(|p| item(p)).collect() };
    let get = |p: &Path| core.lib.lock().unwrap().clips.iter().find(|c| c.path.eq_ignore_ascii_case(&p.to_string_lossy())).cloned();
    let bytes = fs::read(&in_folder).unwrap_or_default();

    // In place with game folders: both are filed in the game's folder, nothing moves.
    let res = run_placed(core, items(&[&in_folder, &elsewhere]), false, Place::Games);
    r.check("in place, game folders: 2 added", res == Ok((2, 0)), format!("{res:?}"));
    let (a, b) = (get(&in_folder), get(&elsewhere));
    r.check(
        "in place, game folders: a clip with a known game is filed like its siblings (the Main folder bug)",
        a.as_ref().is_some_and(|c| c.folder == wf && c.game == want) && b.as_ref().is_some_and(|c| c.folder == wf && c.game == want),
        format!("{:?} / {:?}", a.map(|c| (c.game, c.folder)), b.map(|c| (c.game, c.folder))),
    );
    r.check("in place, game folders: the folder exists in the clips folder and is listed", clips.join(&wf).is_dir() && core.folders().iter().any(|f| *f == wf), format!("{:?}", core.folders()));
    r.check("in place, game folders: originals untouched, nothing copied", fs::read(&in_folder).unwrap_or_default() == bytes && elsewhere.is_file() && fs::read_dir(clips.join(&wf)).map(|d| d.count() == 0).unwrap_or(false), "");
    core.rescan();
    r.check("rescan keeps an in-place clip in its folder", get(&in_folder).is_some_and(|c| c.folder == wf), format!("{:?}", get(&in_folder).map(|c| c.folder)));

    // Folder operations never move a file that stays outside the clips folder.
    let id = get(&in_folder).map(|c| c.id).unwrap_or_default();
    let mv = core.move_clips(&[id.clone()], "Picked");
    r.check("move: an in-place clip is filed, its file stays", mv == Ok(1) && get(&in_folder).is_some_and(|c| c.folder == "Picked") && in_folder.is_file() && fs::read_dir(clips.join("Picked")).map(|d| d.count() == 0).unwrap_or(false), format!("{mv:?}"));
    let rn = core.rename_folder("Picked", "Picked again");
    r.check("rename folder: the in-place clip follows, its path is unchanged", rn.is_ok() && get(&in_folder).is_some_and(|c| c.folder == "Picked again" && Path::new(&c.path) == in_folder.as_path()), format!("{rn:?}"));
    let dl = core.delete_folder("Picked again");
    r.check("delete folder: the in-place clip goes back to the main folder, the original stays", dl.is_ok() && get(&in_folder).is_some_and(|c| c.folder.is_empty()) && fs::read(&in_folder).unwrap_or_default() == bytes && !clips.join("Picked again").exists(), format!("{dl:?}"));
    let sg = core.sort_by_game();
    r.check("sort by game: files an in-place clip without moving it", sg.is_ok() && get(&in_folder).is_some_and(|c| c.folder == wf) && fs::read(&in_folder).unwrap_or_default() == bytes, format!("{sg:?}"));
    let _ = core.move_clips(&[id], "Gone");
    let _ = fs::remove_dir(clips.join("Gone"));
    core.rescan();
    r.check("rescan: a folder removed by hand gives its in-place clips back to the main folder", get(&in_folder).is_some_and(|c| c.folder.is_empty()) && in_folder.is_file(), "");

    // In place into one folder.
    let res = run_placed(core, items(&[&loose_a, &loose_b]), false, Place::One("Medal".into()));
    r.check("in place, one folder: every clip in it", res == Ok((2, 0)) && [&loose_a, &loose_b].iter().all(|p| get(p).is_some_and(|c| c.folder == "Medal") && p.is_file()) && clips.join("Medal").is_dir(), format!("{res:?}"));

    // Copies into one folder that exists with other letter case: its spelling is kept.
    let _ = fs::create_dir_all(clips.join("Best Of"));
    let res = run_placed(core, items(&[&copy_a, &copy_b]), true, Place::One("best of".into()));
    let ca = clips.join("Best Of").join("Apex Legends 2024.06.03 - 10.00.00.00.mp4");
    r.check("copy, one folder: copies there, filed under the existing spelling", res == Ok((2, 0)) && ca.is_file() && get(&ca).is_some_and(|c| c.folder == "Best Of") && copy_a.is_file() && copy_b.is_file(), format!("{res:?} {:?}", get(&ca).map(|c| c.folder)));

    // No folder, with the game folders setting on: the main folder.
    core.settings.lock().unwrap().game_folders = true;
    let res = run_placed(core, items(&[&main_a]), true, Place::Main);
    let cm = clips.join("Deadlock 2024.06.05 - 10.00.00.00.mp4");
    r.check("copy, no folder: in the clips folder itself although game folders is on", res == Ok((1, 0)) && get(&cm).is_some_and(|c| c.folder.is_empty()) && main_a.is_file(), format!("{res:?}"));
    core.settings.lock().unwrap().game_folders = false;
}

/// `glint.exe --import-test <report.txt>`: headless checks on scratch folders.
pub fn self_test(report_path: &str) {
    let mut r = Report { lines: Vec::new() };
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
    } else {
        run_self_test(&mut r);
    }
    let _ = fs::write(report_path, r.lines.join("\n") + "\n");
}

fn run_self_test(r: &mut Report) {
    let base = util::data_dir().join("import-tree");
    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_file(records_file());
    let src = base.join("src");
    let src2 = base.join("src2");
    let clips = src.join("Glint clips");
    let _ = fs::create_dir_all(&clips);
    let _ = fs::create_dir_all(&src2);

    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = clips.to_string_lossy().into_owned();
        s.game_folders = false;
    }
    core.lib.lock().unwrap().clips.clear();
    core.lib.lock().unwrap().save();
    let Some(ffmpeg) = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg") else {
        r.check("ffmpeg found", false, "ffmpeg is not on PATH");
        return;
    };
    let tiny = base.join("tiny.mp4");
    let made = make_video(&ffmpeg, &tiny);
    r.check("made a 1-second test video", made, "ffmpeg lavfi failed");
    if !made {
        return;
    }

    // --- names
    let names = [
        ("Overwatch 2 2024.05.01 - 12.34.56.78.DVR", "Overwatch 2"),
        ("Desktop 2024.05.02 - 09.00.00.01", "Desktop"),
        ("League of Legends 07-14-2020_16-05-00-123", "League of Legends"),
        ("League of Legends_7-14-2020_16-2-34-270", "League of Legends"),
        ("Apex Legends_2024-05-01_12-34-56", "Apex Legends"),
        ("Risk of Rain 2 20240501123456", "Risk of Rain 2"),
        ("Counter-Strike 2", "Counter-Strike 2"),
        ("FIFA 2024", "FIFA 2024"),
        ("MedalTVosu20261111171425661-trim-1794447706596", "MedalTVosu20261111171425661"),
    ];
    for (raw, want) in names {
        let got = clean_name(raw).0;
        r.check(&format!("clean name \"{raw}\""), got == want, format!("got \"{got}\", want \"{want}\""));
    }

    let srcs = sources();
    r.check("sources lists the four programs", srcs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>() == ["shadowplay", "medal", "outplayed", "steelseries"], format!("{srcs:?}"));
    r.check("every source has a folder to show", srcs.iter().all(|s| !s.path.is_empty() && s.found == Path::new(&s.path).is_dir()), format!("{srcs:?}"));
    for s in &srcs {
        r.lines.push(format!("INFO source {}: {} ({})", s.id, s.path, if s.found { "found" } else { "not found" }));
    }

    // --- scratch trees imitating each program
    let put = |rel: &str, secs: u64| -> PathBuf {
        let p = base.join(rel);
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::copy(&tiny, &p);
        set_mtime(&p, secs);
        p
    };
    let t0 = 1_700_000_000u64;
    let nv_a = put(r"src\NVIDIA\Overwatch 2\Overwatch 2 2024.05.01 - 12.34.56.78.DVR.mp4", t0 + 100);
    let nv_b = put(r"src\NVIDIA\Desktop\Desktop 2024.05.02 - 09.00.00.01.DVR.mp4", t0 + 90);
    let nv_c = put(r"src\NVIDIA\Base Profile\Base Profile 2024.05.03 - 10.00.00.00.mp4", t0 + 80);
    let nv_mkv = put(r"src\NVIDIA\Overwatch 2\Overwatch 2 2024.05.04 - 12.00.00.00.mkv", t0 + 70);
    let md_e = put(r"src\Medal\Clips\Risk of Rain 2\MedalTVRiskofRain220261023215833072.mp4", t0 + 60);
    let md_f = put(r"src\Medal\Clips\Screen Recording\MedalTVScreenRecording20261021233549258.mp4", t0 + 50);
    let md_g = put(r"src\Medal\Edits\MedalTVRiskofRain220261023215833072-trim-1794447706596.mp4", t0 + 40);
    let op_i = put(r"src\Overwolf\Outplayed\League of Legends\League of Legends_7-14-2020_16-2-34-270\League of Legends 07-14-2020_16-05-00-123.mp4", t0 + 30);
    let ss_k = put(r"src\SteelSeries Moments\Apex Legends\Apex Legends_2024-05-01_12-34-56.mp4", t0 + 20);
    let ss_l = put(r"src\SteelSeries Moments\Exported Clips\Apex Legends 2024-05-02 10-00-00.mp4", t0 + 10);
    let decoys = [
        put(r"src\Medal\Thumbnails\decoy.mp4", t0),
        put(r"src\Medal\Temp\decoy.mp4", t0),
        put(r"src\Overwolf\Outplayed\tmp\split.mp4", t0),
        put(r"src\SteelSeries Moments\thumbs\t.mp4", t0),
        put(r"src\SteelSeries Moments\.cache\c.mp4", t0),
        put(r"src\Glint clips\Overwatch 2\own clip.mp4", t0),
        put(r"src\Deep\a\b\c\d\e\f\g\deep.mp4", t0),
    ];
    // Windows' hidden attribute on a folder is skipped too.
    let hidden = base.join(r"src\SteelSeries Moments\Hidden stuff");
    let _ = fs::create_dir_all(&hidden);
    let hidden_file = put(r"src\SteelSeries Moments\Hidden stuff\h.mp4", t0);
    let _ = util::hidden_cmd(Path::new("attrib")).arg("+h").arg(&hidden).status();
    let _ = fs::write(base.join(r"src\Medal\Clips\Risk of Rain 2\cover.jpg"), b"jpg");
    // A broken file with a name that parses fine.
    let broken = base.join(r"src\Broken\Broken Game 2024.05.01 - 12.00.00.00.DVR.mp4");
    let _ = fs::create_dir_all(broken.parent().unwrap());
    let _ = fs::write(&broken, vec![0x5a_u8; 4096]);
    set_mtime(&broken, t0 + 5);

    // --- scan of the whole tree
    let found = match scan_with(&core, &src.to_string_lossy(), false) {
        Ok(f) => f,
        Err(e) => {
            r.check("scan the scratch tree", false, e);
            return;
        }
    };
    let game = |p: &Path| found.iter().find(|f| f.path.eq_ignore_ascii_case(&p.to_string_lossy())).map(|f| f.game.clone()).unwrap_or_else(|| "(not found)".into());
    for (what, p, want) in [
        ("ShadowPlay game folder", &nv_a, "Overwatch"), // "Overwatch 2" is filed under the one name (games::canonical)
        ("ShadowPlay mkv", &nv_mkv, "Overwatch"),
        ("ShadowPlay Desktop folder", &nv_b, "Desktop"),
        ("ShadowPlay Base Profile is Desktop", &nv_c, "Desktop"),
        ("Medal game folder", &md_e, "Risk of Rain 2"),
        ("Medal Screen Recording is Desktop", &md_f, "Desktop"),
        ("Medal edit named after a known game", &md_g, "Risk of Rain 2"),
        ("Outplayed nested session folder", &op_i, "League of Legends"),
        ("SteelSeries game folder", &ss_k, "Apex Legends"),
        ("SteelSeries exported clip, by file name", &ss_l, "Apex Legends"),
    ] {
        let got = game(p);
        r.check(&format!("game: {what}"), got == want, format!("got \"{got}\", want \"{want}\""));
    }
    let listed = |p: &Path| found.iter().any(|f| f.path.eq_ignore_ascii_case(&p.to_string_lossy()));
    for d in &decoys {
        r.check(&format!("skipped {}", d.strip_prefix(&src).unwrap_or(d).display()), !listed(d), "it was listed");
    }
    r.check("skipped a hidden folder", !listed(&hidden_file), "it was listed");
    r.check("only videos are listed", found.iter().all(|f| f.path.to_lowercase().ends_with(".mp4") || f.path.to_lowercase().ends_with(".mkv")), "");
    r.check("found every clip (11 with the broken one)", found.len() == 11, format!("found {}", found.len()));
    r.check("sorted newest first", found.windows(2).all(|w| w[0].modified >= w[1].modified), "");
    r.check("modified time is the file's", found.iter().find(|f| f.path.eq_ignore_ascii_case(&nv_a.to_string_lossy())).map(|f| f.modified) == Some((t0 + 100) * 1000), "");
    r.check("nothing is known before an import", found.iter().all(|f| !f.known), "");
    r.check("scanning Glint's own clips folder is refused", scan_with(&core, &clips.to_string_lossy(), false).is_err(), "");
    r.check("scanning a missing folder is an error", scan_with(&core, &base.join("nope").to_string_lossy(), false).is_err(), "");

    // --- file names alone, and Medal's index
    let f_val = put(r"src2\Valorant 2024.06.01 - 10.00.00.00.DVR.mp4", t0 + 9);
    let f_lol = put(r"src2\League of Legends 07-14-2020_16-05-00-123.mp4", t0 + 8);
    let f_apex = put(r"src2\Apex Legends_2024-05-01_12-34-56.mp4", t0 + 7);
    let f_none = put(r"src2\myclip.mp4", t0 + 6);
    let f_idx = put(r"src2\MedalTVDeduction20261112202521505.mp4", t0 + 5);
    let f_naive = put(r"src2\MedalTVSomeGame20261112202521505.mp4", t0 + 4);
    // A stand-in for Medal's index (its record format, as found in medal-<user>.db).
    let mut db: Vec<u8> = b"SQLite format 3\0junk".to_vec();
    db.extend_from_slice(b"\x01\x02C:\\Users\\x\\Videos\\Medal\\Clips\\Deduction\\MedalTVDeduction20261112202521505.mp4");
    db.extend_from_slice(b"C:\\Users\\x\\Videos\\Medal\\Thumbnails\\MedalTVDeduction20261112202521505.jpg\x00\x10");
    db.extend_from_slice(b"\xc7\x0ccategoryName\x97Deduction\x47icon");
    db.extend_from_slice(b"\x00\x05C:\\Users\\x\\Videos\\Medal\\Clips\\osu\\MedalTVosu20261106012227362.mp4\x00\xc7\x0ccategoryName\x47osu!\x47icon");
    let f_osu = put(r"src2\Clips\osu\MedalTVosu20261106012227362.mp4", t0 + 3);
    let medal_root = src.join("Medal");
    let esc = serde_json::to_string(&medal_root.to_string_lossy()).unwrap();
    let esc = esc.trim_matches('"');
    db.extend_from_slice(b"\x21\x46ClipFolderVolumeId\x00ClipFolder\xc8");
    db.push(esc.len() as u8);
    db.extend_from_slice(esc.as_bytes());
    db.extend_from_slice(b"\x00\x00");
    let _ = fs::create_dir_all(src2.join("Databases"));
    let _ = fs::write(src2.join("Databases").join("medal-1.db"), &db);
    let idx = medal_index(&db);
    r.check("Medal index: clip -> game", idx.get("medaltvdeduction20261112202521505.mp4").map(String::as_str) == Some("Deduction"), format!("{idx:?}"));
    let cf = medal_clip_folders(&db);
    r.check("Medal ClipFolder setting is read", cf.iter().any(|p| norm(p) == norm(&medal_root)), format!("{cf:?}"));
    match scan_with(&core, &src2.to_string_lossy(), false) {
        Ok(f2) => {
            let g2 = |p: &Path| f2.iter().find(|f| f.path.eq_ignore_ascii_case(&p.to_string_lossy())).map(|f| f.game.clone()).unwrap_or_else(|| "(not found)".into());
            for (what, p, want) in [
                ("ShadowPlay file name", &f_val, "Valorant"),
                ("Outplayed file name", &f_lol, "League of Legends"),
                ("SteelSeries file name", &f_apex, "Apex Legends"),
                ("Medal index", &f_idx, "Deduction"),
                ("Medal file name without help", &f_naive, "SomeGame"),
                ("unknown is Imported", &f_none, "Imported"),
                ("Medal folder spelled as Medal's index names it", &f_osu, "osu!"),
            ] {
                let got = g2(p);
                r.check(&format!("game: {what}"), got == want, format!("got \"{got}\", want \"{want}\""));
            }
        }
        Err(e) => r.check("scan of loose files", false, e),
    }

    // --- import in place, with a broken file among them
    let pick_items = |list: &[Found], paths: &[&PathBuf]| -> Vec<Found> { list.iter().filter(|f| paths.iter().any(|p| f.path.eq_ignore_ascii_case(&p.to_string_lossy()))).cloned().collect() };
    let before_a = fs::read(&nv_a).unwrap_or_default();
    let before_mtime = fs::metadata(&nv_a).ok().map(|m| mtime_ms(&m));
    let items = pick_items(&found, &[&nv_a, &nv_b, &broken]);
    let res = run_counted(&core, items.clone(), false);
    r.check("in place: 2 added, the broken file skipped", res == Ok((2, 1)), format!("{res:?}"));
    let entry = core.lib.lock().unwrap().clips.iter().find(|c| c.path.eq_ignore_ascii_case(&nv_a.to_string_lossy())).cloned();
    r.check("in place: library entry points at the original", entry.is_some(), "");
    if let Some(c) = &entry {
        r.check("in place: kind is import", c.kind == "import", &c.kind);
        r.check("in place: created is the file's modified time", c.created == (t0 + 100) * 1000, c.created.to_string());
        r.check("in place: game and folder", c.game == "Overwatch" && c.folder.is_empty(), format!("{} / {}", c.game, c.folder));
        r.check("in place: has a thumbnail and a duration", !c.thumb.is_empty() && c.duration > 0.5, format!("{} {}", c.thumb, c.duration));
    }
    r.check("in place: original untouched", fs::read(&nv_a).unwrap_or_default() == before_a && fs::metadata(&nv_a).ok().map(|m| mtime_ms(&m)) == before_mtime, "");
    r.check("in place: broken file left where it was", broken.is_file(), "");
    r.check("in place: game recorded for its chip", core.games.lock().unwrap().list.iter().any(|g| g.name == "Overwatch"), "");
    let again = scan_with(&core, &src.to_string_lossy(), false).unwrap_or_default();
    let is_known = |list: &[Found], p: &Path| list.iter().find(|f| f.path.eq_ignore_ascii_case(&p.to_string_lossy())).map(|f| f.known);
    r.check("known after an import", is_known(&again, &nv_a) == Some(true) && is_known(&again, &nv_b) == Some(true), "");
    r.check("a skipped file is not known", is_known(&again, &broken) == Some(false), "");
    let res = run_counted(&core, items, false);
    r.check("importing twice adds nothing (the broken file is tried and skipped again)", res == Ok((0, 1)), format!("{res:?}"));

    // --- import with copy, game folders on, and a name clash
    core.settings.lock().unwrap().game_folders = true;
    let clash = clips.join("Desktop").join("MedalTVScreenRecording20261021233549258.mp4");
    let _ = fs::create_dir_all(clash.parent().unwrap());
    let _ = fs::write(&clash, b"other");
    let before_e = fs::read(&md_e).unwrap_or_default();
    let items = pick_items(&found, &[&md_e, &md_f]);
    let res = run_counted(&core, items.clone(), true);
    r.check("copy: 2 added", res == Ok((2, 0)), format!("{res:?}"));
    let copy_e = clips.join("Risk of Rain 2").join("MedalTVRiskofRain220261023215833072.mp4");
    r.check("copy: original still there and identical", fs::read(&md_e).unwrap_or_default() == before_e && !before_e.is_empty(), "");
    r.check("copy: copy in the game folder", fs::read(&copy_e).unwrap_or_default() == before_e, copy_e.display().to_string());
    let lib = core.lib.lock().unwrap().clips.clone();
    let ce = lib.iter().find(|c| c.path.eq_ignore_ascii_case(&copy_e.to_string_lossy()));
    r.check("copy: entry points at the copy, folder set", ce.is_some_and(|c| c.folder == "Risk of Rain 2" && c.game == "Risk of Rain 2" && c.kind == "import" && c.created == (t0 + 60) * 1000), format!("{ce:?}"));
    r.check("copy: no overwrite on a name clash", fs::read(&clash).unwrap_or_default() == b"other" && clips.join("Desktop").join("MedalTVScreenRecording20261021233549258 (2).mp4").is_file(), "");
    r.check("copy: clash copy is in the library", lib.iter().any(|c| c.path.ends_with("MedalTVScreenRecording20261021233549258 (2).mp4") && c.folder == "Desktop" && c.game == "Desktop"), "");
    r.check("copy: importing twice adds nothing", run_counted(&core, items, true) == Ok((0, 0)), "");
    let again = scan_with(&core, &src.to_string_lossy(), false).unwrap_or_default();
    r.check("copy: source shows as known", is_known(&again, &md_e) == Some(true), "");
    r.check("copy: Glint's folder still skipped after copying into it", !again.iter().any(|f| norm(Path::new(&f.path)).starts_with(&norm(&clips))), "");

    // --- copy with game folders off
    core.settings.lock().unwrap().game_folders = false;
    let res = run_counted(&core, pick_items(&found, &[&ss_k]), true);
    let copy_k = clips.join("Apex Legends_2024-05-01_12-34-56.mp4");
    r.check("copy without game folders: into the clips folder", res == Ok((1, 0)) && copy_k.is_file() && ss_k.is_file(), format!("{res:?}"));
    r.check("copy without game folders: folder is empty", core.lib.lock().unwrap().clips.iter().any(|c| c.path.eq_ignore_ascii_case(&copy_k.to_string_lossy()) && c.folder.is_empty()), "");

    // --- many at once: workers keep the library consistent
    let rest: Vec<Found> = scan_with(&core, &src.to_string_lossy(), false).unwrap_or_default().into_iter().filter(|f| !f.known).collect();
    let n_rest = rest.len();
    let before = core.lib.lock().unwrap().clips.len();
    let res = run_counted(&core, rest, false);
    let after = core.lib.lock().unwrap().clips.len();
    r.check("bulk import: every readable file added once", res.as_ref().map(|(a, s)| a + s == n_rest && *s == 1).unwrap_or(false) && after == before + res.as_ref().map(|(a, _)| *a).unwrap_or(0), format!("{res:?}, {n_rest} sent, library {before} -> {after}"));
    let paths: Vec<String> = core.lib.lock().unwrap().clips.iter().map(|c| c.path.to_lowercase()).collect();
    let uniq: HashSet<&String> = paths.iter().collect();
    r.check("bulk import: no duplicate entries", uniq.len() == paths.len(), "");
    r.check("all originals still in place", [&nv_a, &nv_b, &nv_c, &nv_mkv, &md_e, &md_f, &md_g, &op_i, &ss_k, &ss_l, &broken].iter().all(|p| p.is_file()), "");

    placement_tests(r, &core, &base, &clips, &put);

    let fails = r.lines.iter().filter(|l| l.starts_with("FAIL")).count();
    // Optional, read-only: how the real folders on this PC would be sorted (counts only, nothing imported).
    if std::env::var_os("GLINT_IMPORT_PROBE").is_some() {
        for s in srcs.iter().filter(|s| s.found) {
            let started = std::time::Instant::now();
            match scan_with(&core, &s.path, true) {
                Ok(list) => {
                    let mut by: Vec<(String, usize)> = Vec::new();
                    for f in &list {
                        match by.iter_mut().find(|(g, _)| *g == f.game) {
                            Some(e) => e.1 += 1,
                            None => by.push((f.game.clone(), 1)),
                        }
                    }
                    let games: Vec<String> = by.iter().map(|(g, n)| format!("{g} x{n}")).collect();
                    r.lines.push(format!("INFO probe {}: {} videos in {} ms: {}", s.id, list.len(), started.elapsed().as_millis(), games.join(", ")));
                }
                Err(e) => r.lines.push(format!("INFO probe {}: {e}", s.id)),
            }
        }
    }
    let checks = r.lines.iter().filter(|l| l.starts_with("PASS") || l.starts_with("FAIL")).count();
    r.lines.push(format!("{checks} checks, {fails} failed"));
    // Leave nothing behind but the report.
    let _ = util::hidden_cmd(Path::new("attrib")).arg("-h").arg(&hidden).status();
    let _ = fs::remove_dir_all(&base);
}
