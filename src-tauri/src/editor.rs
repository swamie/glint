//! Clip editor backend: per-track audio for the editor's preview, and the
//! export that applies a trim, a speed change, color correction and an audio
//! track mix by re-encoding with ffmpeg.
//!
//! The types below are the contract with the UI (`ui/editor.js`). The Tauri
//! commands that call these functions live in `app.rs`.
//!
//! Color semantics (the UI preview must apply the same steps, in this order,
//! to 0..1 sRGB channel values, clamping to 0..1 after every step):
//!   1. brightness b: v * b                      (CSS `brightness(b)`)
//!   2. contrast c:   (v - 0.5) * c + 0.5        (CSS `contrast(c)`)
//!   3. saturation s: the CSS `saturate(s)` matrix (Filter Effects spec,
//!      coefficients 0.213 / 0.715 / 0.072)
//!   4. gamma g:      v ^ (1 / g)                (g > 1 brightens mid-tones)
//! Inputs are clamped first: brightness 0..=3, contrast 0..=3,
//! saturation 0..=2, gamma 0.2..=5.
use crate::library::{self, ClipMeta};
use crate::settings::Settings;
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// One audio track of a clip, extracted to its own file so the editor can play
/// and mix tracks separately (WebView2 cannot switch tracks inside one file).
#[derive(Serialize, Clone, Debug)]
pub struct AudioTrack {
    /// Index among the clip's audio streams: 0 = mix, 1 = desktop, 2 = microphone on a 3-track clip.
    pub index: u32,
    /// The stream's title tag ("Mix", "Desktop", "Microphone"), or "Track N".
    pub title: String,
    /// Extracted audio file the UI can load through the asset protocol.
    pub path: String,
}

/// Color correction. 1.0 is "unchanged" for every field.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Color {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
}

impl Default for Color {
    fn default() -> Self {
        Color { brightness: 1.0, contrast: 1.0, saturation: 1.0, gamma: 1.0 }
    }
}

/// How loud one source audio track is in the export. 0.0 leaves it out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrackVolume {
    pub index: u32,
    pub volume: f32,
}

/// A range of source time, in seconds, and how its picture looks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Range {
    pub start: f64,
    pub end: f64,
    /// This range's color correction; replaces `ExportOpts::color` here. None = that one.
    pub color: Option<Color>,
    /// The picture is black here (same size and frame timing); the audio plays on.
    pub blank: bool,
}

/// A source range where an audio row is louder or quieter than its own volume.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct GainRegion {
    pub start: f64,
    pub end: f64,
    /// Multiplies the row's volume here. 0 = silent, 2 = twice as loud.
    pub gain: f32,
}

/// One audio row of the timeline. All times are source seconds: rows are laid
/// out against the uncut clip, and `ExportOpts::segments` then cuts every row
/// the same way as the video.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct AudioEdit {
    /// "track" = audio stream `index` of the clip; "file" = the audio file at `path`.
    pub source: String,
    pub index: u32,
    pub path: String,
    /// File rows: the source time at which the file's audio starts playing.
    pub offset: f64,
    /// File rows: where in the file to start, and how many seconds of it to use (0 = to its end).
    pub trim_start: f64,
    pub length: f64,
    /// The row's level, 0..=2.
    pub volume: f32,
    pub regions: Vec<GainRegion>,
    /// File rows: the file plays on through every cut (0.1.34). `offset`, `length`
    /// and `regions` are then on the RESULT's timeline (seconds of kept video,
    /// before the speed change), and the row is mixed in after the cuts.
    pub through: bool,
}

/// Loudness outline of one audio stream, for drawing a waveform.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Waveform {
    /// Index among the clip's audio streams.
    pub index: u32,
    /// Peak level per slice of time, 0..1, oldest first.
    pub peaks: Vec<f32>,
    /// How many peaks cover one second.
    pub per_second: f32,
}

/// What the editor needs to know about an audio file the user dropped in.
#[derive(Serialize, Clone, Debug, Default)]
pub struct AudioInfo {
    pub path: String,
    /// File name without its extension.
    pub title: String,
    pub duration: f64,
    pub peaks: Vec<f32>,
    pub per_second: f32,
}

/// Everything the editor's Export button sends.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ExportOpts {
    /// Part of the source to keep, in seconds of source time. `end <= start` means "to the end".
    pub start: f64,
    pub end: f64,
    /// Playback speed of the result: 0.25 ..= 4.0. 1.0 = unchanged.
    pub speed: f64,
    pub color: Color,
    /// Volume per source audio track. Empty = keep the clip's audio tracks as they are.
    pub tracks: Vec<TrackVolume>,
    /// Timeline edit: the source ranges to keep, in order, joined end to end.
    /// Empty = use `start`..`end` (one range). Ranges that look different
    /// (color, blank) may touch: the end of one is the start of the next.
    pub segments: Vec<Range>,
    /// Timeline edit: the audio rows to mix into ONE track. Empty = fall back
    /// to `tracks` (and, if that is empty too, keep the clip's audio as recorded).
    pub audio: Vec<AudioEdit>,
    /// `segments` are played in the order given (clips moved around in an
    /// "Edit together" draft) instead of in source order. They never overlap;
    /// two neighbours in the list merge only when they touch in source time
    /// and look the same. Audio rows stay in source time and are cut the same
    /// way, so each range keeps its own sound.
    pub ordered: bool,
    /// Pictures drawn by the editor (text, shapes, drawings) laid over the
    /// result, in order (later ones on top). Times are OUTPUT seconds.
    pub overlays: Vec<Overlay>,
    /// Boxes whose inside is blurred (censoring), under the overlays.
    pub blurs: Vec<BlurBox>,
    /// Seconds the result fades in from black at its start / out to black at
    /// its end, picture and sound together. 0 = no fade.
    pub fade_in: f64,
    pub fade_out: f64,
    /// An output picture of its own size with pieces of the clip placed on
    /// it (vertical videos). None = the clip's own picture, as before.
    pub canvas: Option<Canvas>,
    /// Keyframes (0.1.27): one track per piece of `canvas`, in the same
    /// order. An empty track (or none at all) = the piece stands still, as
    /// before; otherwise the track replaces the piece's own two boxes.
    pub canvas_tracks: Vec<Vec<PiecePoint>>,
}

impl Default for ExportOpts {
    fn default() -> Self {
        ExportOpts { start: 0.0, end: 0.0, speed: 1.0, color: Color::default(), tracks: Vec::new(), segments: Vec::new(), audio: Vec::new(), ordered: false, overlays: Vec::new(), blurs: Vec::new(), fade_in: 0.0, fade_out: 0.0, canvas: None, canvas_tracks: Vec::new() }
    }
}

/// A point of a moving overlay's or blur box's track (keyframes, 0.1.27): at
/// `t` seconds of the RESULT the thing is moved by `dx`, `dy` (fractions of
/// the picture) and scaled by `sx`, `sy` about its own middle. Between two
/// points it moves evenly, or eases in and out when `e` is set on the first
/// one. Two points at the same time make a jump (a cut between keyframes).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct MovePoint {
    pub t: f64,
    pub dx: f64,
    pub dy: f64,
    pub sx: f64,
    pub sy: f64,
    pub e: bool,
}

impl Default for MovePoint {
    fn default() -> Self {
        MovePoint { t: 0.0, dx: 0.0, dy: 0.0, sx: 1.0, sy: 1.0, e: false }
    }
}

/// A point of a moving canvas piece's track: at `t` seconds of the RESULT
/// its place is `x`, `y`, `w`, `h` (fractions of the canvas), the middle of
/// what it shows is `cx`, `cy` (fractions of the source) and `z` canvas
/// pixels show one source pixel. The source box follows from those at every
/// instant, so the piece is never stretched. `e` as in `MovePoint`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PiecePoint {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub cx: f64,
    pub cy: f64,
    pub z: f64,
    pub e: bool,
}

impl Default for PiecePoint {
    fn default() -> Self {
        PiecePoint { t: 0.0, x: 0.0, y: 0.0, w: 1.0, h: 1.0, cx: 0.5, cy: 0.5, z: 1.0, e: false }
    }
}

/// The export's output picture (0.1.26): `width` x `height` pixels, a
/// background ("blur" = the clip blurred and darkened, "black", or
/// "#rrggbb"), and the pieces of the clip drawn on it in order (last on top).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Canvas {
    /// Pixels; made even and kept to 16..=4096 by the export.
    pub width: f64,
    pub height: f64,
    pub background: String,
    pub pieces: Vec<CanvasPiece>,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas { width: 1080.0, height: 1920.0, background: "black".into(), pieces: Vec::new() }
    }
}

/// A box cut out of the source picture (`sx`, `sy`, `sw`, `sh`: fractions of
/// the source) shown in a box of the canvas (`x`, `y`, `w`, `h`: fractions of
/// the canvas; it may hang over the canvas's edges and is clipped there).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct CanvasPiece {
    pub sx: f64,
    pub sy: f64,
    pub sw: f64,
    pub sh: f64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A transparent PNG the size of the picture (written by `save_overlay_png`),
/// shown from `from` to `to` seconds of the RESULT (after cuts, order and speed).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Overlay {
    pub png: String,
    pub from: f64,
    pub to: f64,
    /// A moving overlay (keyframes, 0.1.27): the PNG is only the item's box,
    /// which lies at `x`, `y`, `w`, `h` (fractions of the picture) before
    /// `track` moves and scales it. An empty track = the PNG is the whole
    /// picture, laid at 0:0 as before.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub track: Vec<MovePoint>,
}

/// A box of the picture that is blurred from `from` to `to` seconds of the
/// result. Position and size are fractions (0..1) of the picture; `strength`
/// is the blur's radius (Gaussian sigma) as a fraction of the picture's height,
/// the same the preview gives CSS `blur()` on the shown picture.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct BlurBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub from: f64,
    pub to: f64,
    pub strength: f64,
    /// Keyframes (0.1.27): the box moves and scales about its middle along
    /// this track. Empty = it stands still.
    pub track: Vec<MovePoint>,
}

/// Where overlay PNGs are written; an export removes the ones it used from here.
fn overlay_dir() -> PathBuf {
    util::tmp_dir().join("overlays")
}

/// Decode standard base64 (with or without padding; whitespace ignored).
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        match chunk.len() {
            4 => out.extend([(n >> 16) as u8, (n >> 8) as u8, n as u8]),
            3 => out.extend([(n >> 16) as u8, (n >> 8) as u8]),
            2 => out.push((n >> 16) as u8),
            _ => return None,
        }
    }
    Some(out)
}

/// The editor's overlay picture (a PNG, as a data URL or plain base64) written
/// to a temp file for the export; returns its path. Files left over from an
/// export that never ran are removed after a day.
pub fn save_overlay_png(data: &str) -> Result<String, String> {
    let b64 = data.split_once("base64,").map(|(_, b)| b).unwrap_or(data);
    let bytes = base64_decode(b64).ok_or("The overlay picture is not valid base64")?;
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("The overlay picture is not a PNG".into());
    }
    let dir = overlay_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if let Ok(list) = fs::read_dir(&dir) {
        for e in list.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(86_400));
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let path = dir.join(format!("{}.png", util::new_id()));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Removes the overlay PNGs an export was given when it ends, however it ends
/// (only files inside `overlay_dir`, never anything else a caller names).
struct OverlayFiles(Vec<PathBuf>);

impl Drop for OverlayFiles {
    fn drop(&mut self) {
        let dir = overlay_dir();
        let dir = fs::canonicalize(&dir).unwrap_or(dir);
        for p in &self.0 {
            if fs::canonicalize(p).is_ok_and(|c| c.starts_with(&dir)) {
                let _ = fs::remove_file(p);
            }
        }
    }
}

const BRIGHTNESS_MAX: f64 = 3.0;
const CONTRAST_MAX: f64 = 3.0;
// colorchannelmixer takes coefficients in -2..2; the saturate matrix's
// blue-blue term (0.072 + 0.928 s) leaves that range just above s = 2.
const SATURATION_MAX: f64 = 2.0;
const GAMMA_RANGE: (f64, f64) = (0.2, 5.0);

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

fn is_neutral(c: &Color) -> bool {
    [c.brightness, c.contrast, c.saturation, c.gamma].iter().all(|v| !v.is_finite() || same(*v as f64, 1.0))
}

/// True if `a` and `b` give the same picture (a non-finite field counts as 1, as in `is_neutral`).
fn same_color(a: &Color, b: &Color) -> bool {
    let f = |v: f32| if v.is_finite() { v as f64 } else { 1.0 };
    [(a.brightness, b.brightness), (a.contrast, b.contrast), (a.saturation, b.saturation), (a.gamma, b.gamma)].iter().all(|&(x, y)| same(f(x), f(y)))
}

fn clamped(v: f32, lo: f64, hi: f64) -> f64 {
    if v.is_finite() { (v as f64).clamp(lo, hi) } else { 1.0 }
}

/// Titles of the audio streams of `file`, in stream order (None = untitled).
fn audio_titles(ffmpeg: &Path, file: &Path) -> Result<Vec<Option<String>>, String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("ffprobe.exe was not found")?;
    let out = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index:stream_tags=title,name", "-of", "default=nw=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not start ffprobe: {e}"))?;
    if !out.status.success() {
        return Err(format!("Could not read the clip: {}", util::last_lines(&out.stderr, 1)));
    }
    // ffmpeg writes an MP4 track title into the track's `name` box and its MP4
    // reader hands it back as the tag "name", not "title"; take either.
    let mut titles: Vec<(Option<String>, Option<String>)> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.starts_with("index=") {
            titles.push((None, None));
            continue;
        }
        let Some(last) = titles.last_mut() else { continue };
        let tag = |v: &str| Some(v.trim().to_string()).filter(|t| !t.is_empty());
        if let Some(t) = line.strip_prefix("TAG:title=") {
            last.0 = tag(t);
        } else if let Some(t) = line.strip_prefix("TAG:name=") {
            last.1 = tag(t);
        }
    }
    Ok(titles.into_iter().map(|(title, name)| title.or(name)).collect())
}

/// Remove other clips' extracted audio once nobody has touched it for a day.
fn remove_stale_dirs(tmp: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(tmp) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("edit-") || name == keep {
            continue;
        }
        let old = e
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(86_400));
        if old {
            // A folder still in use by an open editor fails to delete; it goes next time.
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Extract each audio track of `src` to its own file (cached per clip) for the editor's preview.
pub fn audio_tracks(s: &Settings, src: &ClipMeta) -> Result<Vec<AudioTrack>, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let src_path = Path::new(&src.path);
    let src_time = fs::metadata(src_path).and_then(|m| m.modified()).map_err(|_| "The clip file is missing".to_string())?;
    let tmp = util::tmp_dir();
    let dir_name = format!("edit-{}", util::sanitize_filename(&src.id));
    remove_stale_dirs(&tmp, &dir_name);

    let titles = audio_titles(&ffmpeg, src_path)?;
    if titles.is_empty() {
        return Ok(Vec::new());
    }
    let dir = tmp.join(&dir_name);
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create the editor's temp folder: {e}"))?;
    let final_path = |i: usize| dir.join(format!("a{i}.m4a"));
    let part_path = |i: usize| dir.join(format!("a{i}.part.m4a"));
    let fresh = |p: &Path| fs::metadata(p).is_ok_and(|m| m.len() > 0 && m.modified().is_ok_and(|t| t >= src_time));
    let missing: Vec<usize> = (0..titles.len()).filter(|&i| !fresh(&final_path(i))).collect();

    if !missing.is_empty() {
        // One ffmpeg run with an output per track reads the clip once. Stream copy
        // keeps this to a second or two even for a 10-minute clip.
        let mut cmd = util::hidden_cmd(&ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"]).arg(src_path);
        for &i in &missing {
            cmd.args(["-map", &format!("0:a:{i}"), "-c", "copy", "-movflags", "+faststart"]).arg(part_path(i));
        }
        let out = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
        let ok = out.status.success() && missing.iter().all(|&i| part_path(i).is_file());
        if !ok {
            for &i in &missing {
                let _ = fs::remove_file(part_path(i));
            }
            return Err(format!("Could not read the clip's audio: {}", util::last_lines(&out.stderr, 2)));
        }
        // Written under a temporary name first so a failed run never leaves a
        // truncated file that would later pass the freshness check.
        for &i in &missing {
            fs::rename(part_path(i), final_path(i)).map_err(|e| format!("Cannot prepare audio track {}: {e}", i + 1))?;
        }
    }

    Ok(titles
        .into_iter()
        .enumerate()
        .map(|(i, t)| AudioTrack {
            index: i as u32,
            title: t.unwrap_or_else(|| format!("Track {}", i + 1)),
            path: final_path(i).to_string_lossy().into_owned(),
        })
        .collect())
}

/// Color matrix and range to convert the clip's YUV to RGB and back with.
/// Glint's clips are untagged; WebView2 (Chromium) shows untagged video as
/// BT.709, so that is what the preview the user tuned against looked like.
fn video_colorspace(ffmpeg: &Path, file: &Path) -> (&'static str, &'static str) {
    let mut matrix = "bt709";
    let mut range = "tv";
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return (matrix, range) };
    let Ok(out) = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=color_space,color_range", "-of", "default=nw=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return (matrix, range);
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        match line.split_once('=') {
            Some(("color_space", "bt470bg" | "smpte170m")) => matrix = "bt601",
            Some(("color_range", "pc")) => range = "pc",
            _ => {}
        }
    }
    (matrix, range)
}

/// The filter steps that apply `c` with the semantics in the module doc, or
/// None when `c` changes nothing.
///
/// ffmpeg's `eq` filter is not used: its brightness is additive and its
/// contrast pivots on luma only, so it cannot match CSS. Instead the picture
/// goes to 8-bit RGB, where brightness, contrast and gamma are per-channel
/// curves (`lutrgb`, exact to 1/255) and saturate is the CSS 3x3 matrix
/// (`colorchannelmixer`, which clamps to 0..255 like CSS does).
fn color_filter(c: &Color, matrix: &str, range: &str) -> Option<String> {
    if is_neutral(c) {
        return None;
    }
    let b = clamped(c.brightness, 0.0, BRIGHTNESS_MAX);
    let ct = clamped(c.contrast, 0.0, CONTRAST_MAX);
    let sat = clamped(c.saturation, 0.0, SATURATION_MAX);
    let g = clamped(c.gamma, GAMMA_RANGE.0, GAMMA_RANGE.1);

    // Per-channel curve on 0..255 values, clamped after each step.
    let mut curve = "val".to_string();
    if !same(b, 1.0) {
        curve = format!("clip({curve}*{b:.5},0,255)");
    }
    if !same(ct, 1.0) {
        curve = format!("clip(({curve}-127.5)*{ct:.5}+127.5,0,255)");
    }
    let gamma = |x: &str| format!("255*pow({x}/255,{:.6})", 1.0 / g);
    let lut = |e: &str| format!("lutrgb=r='round({e})':g='round({e})':b='round({e})'");

    let mut steps = vec![format!("scale=in_color_matrix={matrix}:in_range={range}:flags=bicubic+accurate_rnd+full_chroma_int"), "format=rgb24".to_string()];
    if same(sat, 1.0) {
        // No channel mixing: brightness, contrast and gamma fold into one table.
        steps.push(lut(&if same(g, 1.0) { curve } else { gamma(&curve) }));
    } else {
        if curve != "val" {
            steps.push(lut(&curve));
        }
        let (r, gg, bl) = (0.213, 0.715, 0.072);
        steps.push(format!(
            "colorchannelmixer=rr={:.5}:rg={:.5}:rb={:.5}:gr={:.5}:gg={:.5}:gb={:.5}:br={:.5}:bg={:.5}:bb={:.5}",
            r + (1.0 - r) * sat,
            gg - gg * sat,
            bl - bl * sat,
            r - r * sat,
            gg + (1.0 - gg) * sat,
            bl - bl * sat,
            r - r * sat,
            gg - gg * sat,
            bl + (1.0 - bl) * sat,
        ));
        if !same(g, 1.0) {
            steps.push(lut(&gamma("val")));
        }
    }
    steps.push(format!("scale=out_color_matrix={matrix}:out_range={range}:flags=bicubic+accurate_rnd"));
    Some(steps.join(","))
}

/// The picture filter `export` applies when color `c` covers the whole of `file`
/// (one look after the join, then yuv420p), for a render rule's color step
/// (rules.rs), so a rule's look matches the editor's export of it exactly.
/// None when `c` changes nothing.
pub(crate) fn look_filter(ffmpeg: &Path, file: &Path, c: &Color) -> Option<String> {
    let (matrix, range) = video_colorspace(ffmpeg, file);
    color_filter(c, matrix, range).map(|f| format!("{f},format=yuv420p"))
}

/// `atempo` stages for `speed`, each within 0.5..=2.0 (the range where it
/// keeps quality best). "anull" when the speed is unchanged.
fn atempo_chain(speed: f64) -> String {
    let mut left = speed;
    let mut stages = Vec::new();
    while left > 2.0 + 1e-9 {
        stages.push("atempo=2".to_string());
        left /= 2.0;
    }
    while left < 0.5 - 1e-9 {
        stages.push("atempo=0.5".to_string());
        left /= 0.5;
    }
    if !same(left, 1.0) {
        stages.push(format!("atempo={left:.6}"));
    }
    if stages.is_empty() { "anull".into() } else { stages.join(",") }
}

/// Choose the H.264 encoder: the user's encoder setting, or on "auto" the first
/// hardware encoder that can encode two frames (same probe as the recorder).
pub(crate) fn pick_encoder(ffmpeg: &Path, wanted: &str) -> String {
    static CACHE: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);
    if matches!(wanted, "nvenc" | "amf" | "qsv" | "x264") {
        return wanted.to_string();
    }
    if let Ok(c) = CACHE.lock() {
        if let Some((_, e)) = c.as_ref().filter(|(p, _)| p == ffmpeg) {
            return e.clone();
        }
    }
    let mut found = "x264";
    for (name, codec) in [("nvenc", "h264_nvenc"), ("amf", "h264_amf"), ("qsv", "h264_qsv")] {
        let ok = util::hidden_cmd(ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=320x240:r=30", "-frames:v", "2", "-pix_fmt", "yuv420p", "-c:v", codec, "-f", "null", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            found = name;
            break;
        }
    }
    if let Ok(mut c) = CACHE.lock() {
        *c = Some((ffmpeg.to_path_buf(), found.to_string()));
    }
    found.to_string()
}

/// Encoder options for a visually clean export (roughly CRF/CQ 18-20).
pub(crate) fn video_codec_args(enc: &str) -> Vec<String> {
    let a: &[&str] = match enc {
        "nvenc" => &["-c:v", "h264_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", "19", "-b:v", "0", "-forced-idr", "1"],
        "amf" => &["-c:v", "h264_amf", "-quality", "quality", "-rc", "cqp", "-qp_i", "18", "-qp_p", "20", "-qp_b", "20"],
        "qsv" => &["-c:v", "h264_qsv", "-preset", "medium", "-global_quality", "20"],
        _ => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"],
    };
    a.iter().map(|s| s.to_string()).collect()
}

/// Run ffmpeg with `-progress pipe:1` and report 0..100 against `expected` seconds of output.
fn run_with_progress(ffmpeg: &Path, args: &[String], expected: f64, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    let mut child = util::hidden_cmd(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    // Drained on its own thread so a chatty stderr can never block ffmpeg while we read progress.
    let err_reader = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let mut last = -1.0f32;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) else { continue };
            if expected > 0.0 {
                // 100 is sent only once the file is complete.
                let pct = ((us / 1e6 / expected) * 100.0).clamp(0.0, 99.0) as f32;
                if pct >= last + 0.5 {
                    last = pct;
                    progress(pct);
                }
            }
        }
    }
    let status = child.wait().map_err(|e| format!("ffmpeg failed: {e}"))?;
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let tail = util::last_lines(&stderr, 2);
        Err(if tail.is_empty() { format!("ffmpeg stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail })
    }
}

/// Sample rate of the timeline mix (`ExportOpts::audio`).
const MIX_RATE: u32 = 48_000;

/// One kept source range `a`..`b` and how its picture looks in the result.
#[derive(Clone, Debug)]
struct Kept {
    a: f64,
    b: f64,
    /// The color that applies here: the range's own, else `ExportOpts::color`
    /// (neutral on a blank range, where nothing of the picture is left to correct).
    color: Color,
    blank: bool,
}

impl Kept {
    fn looks_like(&self, o: &Kept) -> bool {
        self.blank == o.blank && same_color(&self.color, &o.color)
    }
}

/// The source ranges to keep: `segments` clamped into the clip and sorted,
/// with ranges that touch or overlap merged when they look the same; or the
/// one range `start`..`end` when there are none. A range that looks different
/// from the one before starts exactly where that one ends: an overlap goes to
/// the earlier range, and a gap under 1 ms (rounding in the UI) is closed.
/// With `ordered` the list keeps its order (see `ExportOpts::ordered`).
fn kept_ranges(opts: &ExportOpts, duration: f64) -> Vec<Kept> {
    if opts.segments.is_empty() {
        let start = if opts.start.is_finite() { opts.start.clamp(0.0, duration) } else { 0.0 };
        let to_end = !(opts.end.is_finite() && opts.end > start) || opts.end >= duration;
        return vec![Kept { a: start, b: if to_end { duration } else { opts.end }, color: opts.color.clone(), blank: false }];
    }
    let mut r: Vec<Kept> = opts
        .segments
        .iter()
        .filter(|g| g.start.is_finite() && g.end.is_finite())
        .map(|g| Kept {
            a: g.start.clamp(0.0, duration),
            b: g.end.clamp(0.0, duration),
            color: if g.blank { Color::default() } else { g.color.clone().unwrap_or_else(|| opts.color.clone()) },
            blank: g.blank,
        })
        .filter(|k| k.b - k.a > 0.001)
        .collect();
    if opts.ordered {
        // In the order given: only list neighbours that touch in source time join up.
        let mut out: Vec<Kept> = Vec::new();
        for mut k in r {
            if let Some(last) = out.last_mut() {
                if (k.a - last.b).abs() <= 1e-3 {
                    if k.looks_like(last) {
                        last.b = k.b;
                        continue;
                    }
                    k.a = last.b;
                }
            }
            out.push(k);
        }
        return out;
    }
    // Stable: of two ranges that start together, the one sent first wins.
    r.sort_by(|x, y| x.a.total_cmp(&y.a));
    let mut out: Vec<Kept> = Vec::new();
    for mut k in r {
        if let Some(last) = out.last_mut() {
            if k.a <= last.b + 1e-3 {
                if k.looks_like(last) {
                    last.b = last.b.max(k.b);
                    continue;
                }
                k.a = last.b;
                if k.b - k.a <= 0.001 {
                    continue;
                }
            }
        }
        out.push(k);
    }
    out
}

/// Where one row of the timeline mix gets its audio.
enum Source {
    /// Audio stream `.0` of the clip, which has `.1` channels.
    Track(usize, u32),
    /// `len` seconds of `path` from `seek`, heard from `at` seconds after the first kept range.
    File { path: String, seek: f64, len: f64, at: f64, channels: u32 },
}

struct MixRow {
    source: Source,
    volume: f64,
    /// (start, end, gain) on the mix timeline (0 = the first kept range's start).
    regions: Vec<(f64, f64, f64)>,
    /// A file that plays through cuts: `at` and `regions` are on the result's timeline
    /// (kept seconds), and it is mixed in after the cuts.
    through: bool,
}

enum AudioPlan {
    /// No audio stream in the result.
    Silent,
    /// The clip's tracks as they are, stream-copied (one range, normal speed).
    Copy,
    /// Every track of the clip, each cut and sped up like the video.
    Tracks,
    /// One track: the mix of `rows`.
    Mix { rows: Vec<MixRow>, title: String },
}

/// Channel counts of the audio streams of `file`, and its duration (0 = unknown).
struct AudioProbe {
    channels: Vec<u32>,
    duration: f64,
}

fn probe_audio(ffmpeg: &Path, file: &Path) -> Result<AudioProbe, String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("ffprobe.exe was not found")?;
    let out = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=channels:format=duration", "-of", "default=nw=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not start ffprobe: {e}"))?;
    if !out.status.success() {
        let why = util::last_lines(&out.stderr, 1);
        return Err(if why.is_empty() { "it is not a format ffmpeg can read".into() } else { why });
    }
    let mut p = AudioProbe { channels: Vec::new(), duration: 0.0 };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        match line.split_once('=') {
            Some(("channels", v)) => p.channels.push(v.trim().parse().unwrap_or(2)),
            Some(("duration", v)) => p.duration = v.trim().parse::<f64>().ok().filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0),
            _ => {}
        }
    }
    Ok(p)
}

/// An audio file the mix needs: it must exist and hold audio.
fn check_audio_file(ffmpeg: &Path, path: &str) -> Result<AudioProbe, String> {
    let p = Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    if path.trim().is_empty() || !p.is_file() {
        return Err(format!("The audio file \"{name}\" is missing (was it moved or deleted?)"));
    }
    let info = probe_audio(ffmpeg, p).map_err(|e| format!("Could not read the audio file \"{name}\": {e}"))?;
    if info.channels.is_empty() {
        return Err(format!("The file \"{name}\" has no audio in it"));
    }
    Ok(info)
}

/// Decide what the result's audio is, and check every file it uses.
/// `base` is the lowest kept source time and `total` the span from it up to the highest.
#[allow(clippy::too_many_arguments)]
fn audio_plan(ffmpeg: &Path, src_path: &Path, opts: &ExportOpts, titles: &[Option<String>], duration: f64, base: f64, total: f64, ranges: usize, speed: f64, kept: f64) -> Result<AudioPlan, String> {
    let n_audio = titles.len();
    let level = |v: f32, hi: f64| if v.is_finite() { (v as f64).clamp(0.0, hi) } else { 0.0 };
    let clip_channels = || probe_audio(ffmpeg, src_path).map(|p| p.channels).unwrap_or_default();
    let track_title = |i: usize| titles.get(i).cloned().flatten().unwrap_or_else(|| "Audio".into());

    if opts.audio.is_empty() {
        if opts.tracks.is_empty() {
            return Ok(if n_audio == 0 {
                AudioPlan::Silent
            } else if ranges == 1 && same(speed, 1.0) {
                AudioPlan::Copy
            } else {
                AudioPlan::Tracks
            });
        }
        // The older per-track volume list (no timeline): volumes up to 4 as before.
        let ch = clip_channels();
        let mut rows: Vec<MixRow> = Vec::new();
        for t in &opts.tracks {
            let i = t.index as usize;
            let v = level(t.volume, 4.0);
            if i < n_audio && v > 0.0005 && !rows.iter().any(|r| matches!(r.source, Source::Track(p, _) if p == i)) {
                rows.push(MixRow { source: Source::Track(i, ch.get(i).copied().unwrap_or(2)), volume: v, regions: Vec::new(), through: false });
            }
        }
        let title = match rows.as_slice() {
            [] => return Ok(AudioPlan::Silent),
            [MixRow { source: Source::Track(i, _), .. }] => track_title(*i),
            _ => "Mix".into(),
        };
        return Ok(AudioPlan::Mix { rows, title });
    }

    let ch = clip_channels();
    let fin = |v: f64| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let mut rows: Vec<MixRow> = Vec::new();
    let mut file_title = String::new();
    for e in &opts.audio {
        let volume = level(e.volume, 2.0);
        if volume < 0.0005 {
            continue;
        }
        let regions: Vec<(f64, f64, f64)> = e
            .regions
            .iter()
            .filter(|g| g.start.is_finite() && g.end.is_finite())
            .map(|g| (g.start.clamp(0.0, duration) - base, g.end.clamp(0.0, duration) - base, level(g.gain, 2.0)))
            .filter(|(a, b, _)| b - a > 1e-4 && *b > 0.0)
            .collect();
        match e.source.as_str() {
            "file" => {
                let info = check_audio_file(ffmpeg, &e.path)?;
                let fdur = info.duration;
                let mut seek = fin(e.trim_start);
                if fdur > 0.0 {
                    seek = seek.min(fdur);
                }
                let mut len = if fin(e.length) > 0.0 { fin(e.length) } else if fdur > 0.0 { fdur - seek } else { f64::INFINITY };
                if fdur > 0.0 {
                    len = len.min(fdur - seek);
                }
                if e.through {
                    // Plays on through every cut: placed on the result's own timeline (kept
                    // seconds) and mixed in after the cuts; it stops where the result ends.
                    let at = fin(e.offset).min(kept);
                    len = len.min(kept - at);
                    if len < 0.001 {
                        continue;
                    }
                    let regions: Vec<(f64, f64, f64)> = e
                        .regions
                        .iter()
                        .filter(|g| g.start.is_finite() && g.end.is_finite())
                        .map(|g| (g.start.clamp(0.0, kept), g.end.clamp(0.0, kept), level(g.gain, 2.0)))
                        .filter(|(a, b, _)| b - a > 1e-4)
                        .collect();
                    file_title = Path::new(&e.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    rows.push(MixRow { source: Source::File { path: e.path.clone(), seek, len, at, channels: info.channels[0] }, volume, regions, through: true });
                    continue;
                }
                // Placed on the mix timeline; a row that starts before the first
                // kept range loses the part that would come before it.
                let mut at = fin(e.offset).min(duration) - base;
                if at < 0.0 {
                    seek -= at;
                    len += at;
                    at = 0.0;
                }
                len = len.min(total - at);
                if len < 0.001 || at >= total {
                    continue;
                }
                file_title = Path::new(&e.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                rows.push(MixRow { source: Source::File { path: e.path.clone(), seek, len, at, channels: info.channels[0] }, volume, regions, through: false });
            }
            "track" => {
                let i = e.index as usize;
                if i < n_audio {
                    rows.push(MixRow { source: Source::Track(i, ch.get(i).copied().unwrap_or(2)), volume, regions, through: false });
                }
            }
            _ => {}
        }
    }
    // A file row covering several moved clips comes as one entry per clip: still one source.
    let file_of = |r: &MixRow| match &r.source {
        Source::File { path, .. } => Some(path.clone()),
        Source::Track(..) => None,
    };
    let one_file = !file_title.is_empty() && !rows.is_empty() && rows.iter().all(|r| file_of(r).is_some() && file_of(r) == file_of(&rows[0]));
    let title = match rows.as_slice() {
        [] => return Ok(AudioPlan::Silent),
        [MixRow { source: Source::Track(i, _), .. }] => track_title(*i),
        [MixRow { source: Source::File { .. }, .. }] if !file_title.is_empty() => file_title,
        _ if one_file => file_title,
        _ => "Mix".into(),
    };
    Ok(AudioPlan::Mix { rows, title })
}

/// `input` cut to the kept ranges (given on the input's own timeline, `None` =
/// to its end), joined end to end, then `tempo`. Audio is cut by sample, so a
/// join is a plain hard cut with nothing added or lost.
fn cut_chain(input: &str, ranges: &[(f64, Option<f64>)], tag: &str, tempo: &str, out: &str) -> String {
    let k = ranges.len();
    if k == 1 {
        // The input is already limited to the one range.
        return format!("{input}{tempo}{out}");
    }
    let mut c = format!("{input}asplit={k}");
    for i in 0..k {
        c += &format!("[{tag}c{i}]");
    }
    for (i, (a, b)) in ranges.iter().enumerate() {
        let end = b.map(|b| format!(":end={b:.6}")).unwrap_or_default();
        c += &format!(";[{tag}c{i}]atrim=start={a:.6}{end},asetpts=PTS-STARTPTS[{tag}s{i}]");
    }
    c += ";";
    for i in 0..k {
        c += &format!("[{tag}s{i}]");
    }
    c + &format!("concat=n={k}:v=0:a=1,{tempo}{out}")
}

/// A row's level: its volume, times each region's gain inside that region.
fn gain_filter(volume: f64, regions: &[(f64, f64, f64)]) -> String {
    if regions.is_empty() {
        return if same(volume, 1.0) { "anull".into() } else { format!("volume={volume:.4}") };
    }
    // The first region listed wins where two overlap.
    let mut e = "1".to_string();
    for (a, b, g) in regions.iter().rev() {
        e = format!("if(gte(t,{a:.4})*lt(t,{b:.4}),{g:.4},{e})");
    }
    // `volume` evaluates its expression once per frame; short frames put a
    // region's edge within 2.7 ms of where it was set.
    format!("asetnsamples=n=128:p=0,volume=volume='{volume:.4}*{e}':eval=frame")
}

/// The file's start time: packet times are on the streams' clock, while `-ss`
/// and `-read_intervals` positions count from the file's start.
fn start_time(ffprobe: &Path, file: &Path) -> f64 {
    util::hidden_cmd(ffprobe)
        .args(["-v", "error", "-show_entries", "format=start_time", "-of", "default=nw=1:nk=1"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

/// (time from the file's start, is a keyframe) of each video packet ffprobe
/// reads for `intervals` (its `-read_intervals` syntax), in file order.
fn video_packets(ffprobe: &Path, file: &Path, start: f64, intervals: &str) -> Option<Vec<(f64, bool)>> {
    let out = util::hidden_cmd(ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0", "-read_intervals", intervals])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (pts, flags) = l.split_once(',').unwrap_or((l, ""));
                Some((pts.trim().parse::<f64>().ok()? - start, flags.contains('K')))
            })
            .collect(),
    )
}

/// For each of `times` (seconds from the start of `file`), the time of the
/// video frame on screen at that moment: the last frame at or before it. None
/// where none was found (the caller then takes the first frame after).
pub(crate) fn frames_on_screen(ffmpeg: &Path, file: &Path, times: &[f64]) -> Vec<Option<f64>> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return vec![None; times.len()] };
    if times.is_empty() {
        return Vec::new();
    }
    let start = start_time(&ffprobe, file);
    let one = |t: f64| -> Option<f64> {
        // ffprobe starts reading at a keyframe NEAR the window's start, which
        // can be after `t` when a still screen sits before it; then the window
        // grows until a frame turns up. Packet times only: nothing is decoded.
        for back in [2.0, 20.0, f64::INFINITY] {
            let from = (t - back).max(0.0);
            let packets = video_packets(&ffprobe, file, start, &format!("{:.6}%{:.6}", start + from, start + t + 0.25))?;
            let best = packets.iter().map(|p| p.0).filter(|p| *p <= t + 1e-4).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
            if best.is_some() || from <= 0.0 {
                return best;
            }
        }
        None
    };
    let one = &one;
    let mut found = vec![None; times.len()];
    // Several at once: each is a short ffprobe run reading a couple of seconds of packets.
    std::thread::scope(|sc| {
        for (ts, slots) in times.chunks(8).zip(found.chunks_mut(8)) {
            let runs: Vec<_> = ts.iter().map(|&t| sc.spawn(move || one(t))).collect();
            for (slot, run) in slots.iter_mut().zip(runs) {
                *slot = run.join().ok().flatten();
            }
        }
    });
    found
}

/// The picture layers of an export, checked and clamped to the result's length.
struct Layers {
    /// (png path, from, to) in output seconds, and how it moves (keyframes).
    overlays: Vec<(String, f64, f64, Option<Moving>)>,
    /// (x, y, w, h fractions, from, to, strength), and how it moves.
    blurs: Vec<(f64, f64, f64, f64, f64, f64, f64, Option<Moving>)>,
    fade_in: f64,
    fade_out: f64,
}

/// A moving overlay or blur box (keyframes): its box before it moves (x, y,
/// w, h: fractions of the picture) and its track, checked: (t, dx, dy, sx,
/// sy, ease) sorted by time.
#[derive(Debug, Clone, PartialEq)]
struct Moving {
    bx: (f64, f64, f64, f64),
    pts: Vec<(f64, [f64; 4], bool)>,
}

/// The most points a track keeps (the UI sends a few per keyframe).
const MAX_TRACK: usize = 2000;

/// `pts` checked for a result `total` seconds long: times inside it and in
/// order, moves within 4 pictures, scales 0.01..=50. None when empty.
fn clean_moves(pts: &[MovePoint], total: f64) -> Option<Vec<(f64, [f64; 4], bool)>> {
    let f = |v: f64, d: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
    let mut out: Vec<(f64, [f64; 4], bool)> = pts
        .iter()
        .take(MAX_TRACK)
        .filter(|p| p.t.is_finite())
        .map(|p| (p.t.clamp(0.0, total.max(0.0)), [f(p.dx, 0.0, -4.0, 4.0), f(p.dy, 0.0, -4.0, 4.0), f(p.sx, 1.0, 0.01, 50.0), f(p.sy, 1.0, 0.01, 50.0)], p.e))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    (!out.is_empty()).then_some(out)
}

/// A number for an ffmpeg expression: at most 6 decimals, no trailing zeros.
fn ex_num(v: f64) -> String {
    let s = format!("{:.6}", if v.is_finite() { v } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// One value of a track as an ffmpeg expression on the result's time `t`:
/// the first value, plus for each step between two points the change times
/// how far `t` has got through it (0..1, eased with smoothstep when the first
/// point says so), so the value is held before the first point and after the
/// last. Two points at one time jump at that time. A sum, not nested `if`s,
/// and grouped in parentheses eight at a time: ffmpeg refuses an expression
/// tree about a hundred deep (a plain chain of 100 `+` is one), so the
/// grouping keeps it a few levels deep for any number of points.
fn track_expr(pts: &[(f64, f64, bool)]) -> String {
    let Some(first) = pts.first() else { return "0".into() };
    fn grouped(terms: &[String]) -> String {
        if terms.len() <= 8 {
            return terms.concat();
        }
        let k = terms.len().div_ceil(8);
        terms.chunks(k).map(|c| format!("+({})", grouped(c))).collect()
    }
    let mut terms: Vec<String> = Vec::new();
    for w in pts.windows(2) {
        let ((ta, va, ease), (tb, vb, _)) = (w[0], w[1]);
        let d = vb - va;
        if d.abs() < 1e-7 {
            continue;
        }
        let (sign, mag) = (if d < 0.0 { '-' } else { '+' }, ex_num(d.abs()));
        terms.push(if tb - ta < 1e-6 {
            format!("{sign}{mag}*gte(t,{})", ex_num(ta))
        } else {
            let u = format!("clip((t-{})/{},0,1)", ex_num(ta), ex_num(tb - ta));
            if ease { format!("{sign}{mag}*{u}*{u}*(3-2*{u})") } else { format!("{sign}{mag}*{u}") }
        });
    }
    ex_num(first.1) + &grouped(&terms)
}

/// Field `k` of a checked move track as an expression (`track_expr`).
fn move_expr(pts: &[(f64, [f64; 4], bool)], k: usize) -> String {
    track_expr(&pts.iter().map(|(t, v, e)| (*t, v[k], *e)).collect::<Vec<_>>())
}

/// The value of a track at time `t`, as `track_expr`'s expression gives it (for the self-test).
fn track_value(pts: &[(f64, f64, bool)], t: f64) -> f64 {
    let Some(first) = pts.first() else { return 0.0 };
    let mut v = first.1;
    for w in pts.windows(2) {
        let ((ta, va, ease), (tb, vb, _)) = (w[0], w[1]);
        if tb - ta < 1e-6 {
            if t >= ta {
                v += vb - va;
            }
        } else {
            let u = ((t - ta) / (tb - ta)).clamp(0.0, 1.0);
            v += (vb - va) * if ease { u * u * (3.0 - 2.0 * u) } else { u };
        }
    }
    v
}

/// `opts`' overlays, blur boxes and fades on a result `total` seconds long;
/// None when there are none (or none that shows). A missing overlay file is an error.
fn layer_plan(opts: &ExportOpts, total: f64) -> Result<Option<Layers>, String> {
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let span = |a: f64, b: f64| {
        let a = fin(a).clamp(0.0, total);
        let b = fin(b).clamp(0.0, total);
        (b - a > 0.001).then_some((a, b))
    };
    let mut overlays = Vec::new();
    for o in &opts.overlays {
        let Some((a, b)) = span(o.from, o.to) else { continue };
        if !Path::new(&o.png).is_file() {
            return Err("An overlay picture of the edit is missing. Try the export again.".into());
        }
        // A moving one: the PNG is the item's box (it may hang over the picture's edges).
        let moving = clean_moves(&o.track, total).and_then(|pts| {
            let (w, h) = (fin(o.w).min(4.0), fin(o.h).min(4.0));
            (w > 0.0005 && h > 0.0005).then(|| Moving { bx: (fin(o.x).clamp(-4.0, 4.0), fin(o.y).clamp(-4.0, 4.0), w, h), pts })
        });
        overlays.push((o.png.clone(), a, b, moving));
    }
    let mut blurs = Vec::new();
    for g in &opts.blurs {
        let Some((a, b)) = span(g.from, g.to) else { continue };
        let st = fin(g.strength).clamp(0.0, 0.2);
        if let Some(pts) = clean_moves(&g.track, total) {
            // A moving box: kept whole (its track may take it partly off the picture).
            let (x, y) = (fin(g.x).clamp(-1.0, 2.0), fin(g.y).clamp(-1.0, 2.0));
            let (w, h) = (fin(g.w).min(1.0), fin(g.h).min(1.0));
            if w > 0.002 && h > 0.002 && st > 0.0 {
                blurs.push((x, y, w, h, a, b, st, Some(Moving { bx: (x, y, w, h), pts })));
            }
            continue;
        }
        let x = fin(g.x).clamp(0.0, 1.0);
        let y = fin(g.y).clamp(0.0, 1.0);
        let w = fin(g.w).min(1.0 - x);
        let h = fin(g.h).min(1.0 - y);
        if w > 0.002 && h > 0.002 && st > 0.0 {
            blurs.push((x, y, w, h, a, b, st, None));
        }
    }
    // Fades longer than half the result would overlap: each is kept to half.
    let fade_in = fin(opts.fade_in).clamp(0.0, total / 2.0);
    let fade_out = fin(opts.fade_out).clamp(0.0, total / 2.0);
    let fade_in = if fade_in >= 0.01 { fade_in } else { 0.0 };
    let fade_out = if fade_out >= 0.01 { fade_out } else { 0.0 };
    if overlays.is_empty() && blurs.is_empty() && fade_in == 0.0 && fade_out == 0.0 {
        return Ok(None);
    }
    Ok(Some(Layers { overlays, blurs, fade_in, fade_out }))
}

/// Filter chains from `[vb]` (the joined result, on output time) to `[v]`:
/// each blur box (crop, Gaussian blur, laid back at its place), then each
/// overlay picture (input `first_png` + i, scaled to the picture `size`),
/// each only `between` its times, then the fades. Pixel boxes are even, as
/// 4:2:0 needs, so the blurred crop lands exactly where it was taken.
fn layer_chain(l: &Layers, first_png: usize, (pw, ph): (u32, u32), total: f64) -> Vec<String> {
    let mut g = Vec::new();
    let mut cur = "vb".to_string();
    let even = |v: f64| ((v / 2.0).round() as i64 * 2).max(0) as u32;
    for (i, &(x, y, w, h, a, b, st, ref moving)) in l.blurs.iter().enumerate() {
        if let Some(mv) = moving {
            g.extend(moving_blur_chain(mv, &cur, i, (pw, ph), (a, b, st)));
            cur = format!("vl{i}");
            continue;
        }
        let bx = even(x * pw as f64).min(pw.saturating_sub(2));
        let by = even(y * ph as f64).min(ph.saturating_sub(2));
        let bw = even(w * pw as f64).clamp(2, pw - bx);
        let bh = even(h * ph as f64).clamp(2, ph - by);
        let sigma = (st * ph as f64).clamp(0.5, 500.0);
        let on = format!("enable='between(t,{a:.4},{b:.4})'");
        g.push(format!(
            "[{cur}]split=2[{cur}m][{cur}s];[{cur}s]crop={bw}:{bh}:{bx}:{by},gblur=sigma={sigma:.3}:steps=2:{on}[bb{i}];[{cur}m][bb{i}]overlay={bx}:{by}:{on}[vl{i}]"
        ));
        cur = format!("vl{i}");
    }
    for (j, (_, a, b, moving)) in l.overlays.iter().enumerate() {
        let input = first_png + j;
        if let Some(mv) = moving {
            // The item's box, scaled about its middle and moved per frame (its input is the PNG looped, so a frame comes for every size).
            let (bx, by, bw, bh) = mv.bx;
            let (sx, sy, dx, dy) = (move_expr(&mv.pts, 2), move_expr(&mv.pts, 3), move_expr(&mv.pts, 0), move_expr(&mv.pts, 1));
            let (w0, h0) = (ex_num(pw as f64 * bw), ex_num(ph as f64 * bh));
            let (mx, my) = (ex_num(bx + bw / 2.0), ex_num(by + bh / 2.0));
            g.push(format!(
                "[{input}:v]format=rgba,scale=w='max(2,{w0}*({sx}))':h='max(2,{h0}*({sy}))':eval=frame[op{j}];[{cur}][op{j}]overlay=x='{pw}*({mx}+({dx}))-overlay_w/2':y='{ph}*({my}+({dy}))-overlay_h/2':shortest=1:enable='between(t,{a:.4},{b:.4})'[vo{j}]"
            ));
        } else {
            g.push(format!("[{input}:v]scale={pw}:{ph},format=rgba[op{j}];[{cur}][op{j}]overlay=0:0:enable='between(t,{a:.4},{b:.4})'[vo{j}]"));
        }
        cur = format!("vo{j}");
    }
    let mut tail = vec![];
    if l.fade_in > 0.0 {
        tail.push(format!("fade=t=in:st=0:d={:.4}", l.fade_in));
    }
    if l.fade_out > 0.0 {
        tail.push(format!("fade=t=out:st={:.4}:d={:.4}", (total - l.fade_out).max(0.0), l.fade_out));
    }
    tail.push("format=yuv420p".into());
    g.push(format!("[{cur}]{}[v]", tail.join(",")));
    g
}

/// A moving blur box `i` from `[cur]` to `[vl{i}]` on a `pw` x `ph` picture,
/// shown from `a` to `b` seconds with blur `st` (fraction of the height).
/// `crop` takes a new position every frame but its size only once, so the
/// blurred copy is scaled every frame to make the box a FIXED size (the
/// smallest it gets), the box is cut out of that by laying the copy on a
/// frame of that size at minus the box's position (`overlay` follows each
/// frame's size, `crop` would not), scaled to the box's size of the moment
/// and laid at its place.
fn moving_blur_chain(mv: &Moving, cur: &str, i: usize, (pw, ph): (u32, u32), (a, b, st): (f64, f64, f64)) -> Vec<String> {
    let (bx, by, bw, bh) = mv.bx;
    let even = |v: f64| (((v / 2.0).round() as i64) * 2).max(2) as u32;
    let min_of = |k: usize| mv.pts.iter().map(|p| p.1[k]).fold(f64::INFINITY, f64::min);
    let fw = even(bw * min_of(2) * pw as f64).min(pw & !1);
    let fh = even(bh * min_of(3) * ph as f64).min(ph & !1);
    let (sx, sy, dx, dy) = (move_expr(&mv.pts, 2), move_expr(&mv.pts, 3), move_expr(&mv.pts, 0), move_expr(&mv.pts, 1));
    // The box's size (fractions of the picture) and its left / top edge at time t.
    let (wf, hf) = (format!("{}*({sx})", ex_num(bw)), format!("{}*({sy})", ex_num(bh)));
    let lx = format!("{}+({dx})-{wf}/2", ex_num(bx + bw / 2.0));
    let ty = format!("{}+({dy})-{hf}/2", ex_num(by + bh / 2.0));
    let sigma = (st * ph as f64).clamp(0.5, 500.0);
    let on = format!("enable='between(t,{a:.4},{b:.4})'");
    vec![format!(
        "[{cur}]split=2[{cur}m][{cur}s];[{cur}s]gblur=sigma={sigma:.3}:steps=2:{on},split=2[kb{i}a][kb{i}b];[kb{i}a]crop={fw}:{fh}:0:0[kb{i}c];\
         [kb{i}b]scale=w='max({fw},{fw}/({wf}))':h='max({fh},{fh}/({hf}))':eval=frame[kb{i}d];\
         [kb{i}c][kb{i}d]overlay=x='-clip(({lx})*overlay_w,0,overlay_w-{fw})':y='-clip(({ty})*overlay_h,0,overlay_h-{fh})'[kb{i}e];\
         [kb{i}e]scale=w='max(2,{pw}*{wf})':h='max(2,{ph}*{hf})':eval=frame[bb{i}];\
         [{cur}m][bb{i}]overlay=x='{pw}*({lx})':y='{ph}*({ty})':{on}[vl{i}]"
    )]
}

/// A `Canvas` checked and turned into whole pixels.
#[derive(Debug, Clone, PartialEq)]
struct CanvasPlan {
    width: u32,
    height: u32,
    /// None = the clip blurred and darkened; Some = this flat color (R, G, B).
    color: Option<(u8, u8, u8)>,
    /// Each piece that shows: the crop of the source (x, y, w, h) and the
    /// part of the canvas it covers (x, y, w, h), already clipped to the
    /// canvas. All even, as 4:2:0 needs.
    pieces: Vec<([u32; 4], [u32; 4])>,
    /// Moving pieces (keyframes): each drawn just before `pieces[k]` (after
    /// the last one when k == pieces.len()), so the order is the canvas's.
    moving: Vec<(usize, MovingPiece)>,
}

/// A moving canvas piece, checked: (t, [x, y, w, h, cx, cy, z], ease) sorted
/// by time, and the fixed size (`fw` x `fh` pixels) its source box is
/// scaled to on the way (see `moving_piece_chain`).
#[derive(Debug, Clone, PartialEq)]
struct MovingPiece {
    pts: Vec<(f64, [f64; 7], bool)>,
    fw: u32,
    fh: u32,
}

/// Largest picture a moving piece's source is scaled to on the way, per side.
const MOVING_MAX_SIDE: f64 = 8192.0;

/// A piece's track checked for a `cw` x `ch` canvas on a `sw` x `sh` source:
/// places within 4 canvases, sides 0.005..=8, middles inside the source,
/// zoom 0.01..=100; times in order. None when empty.
fn moving_piece(pts: &[PiecePoint], (cw, ch): (u32, u32), (sw, sh): (u32, u32)) -> Option<MovingPiece> {
    let f = |v: f64, d: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
    let mut list: Vec<(f64, [f64; 7], bool)> = pts
        .iter()
        .take(MAX_TRACK)
        .filter(|p| p.t.is_finite())
        .map(|p| (p.t.max(0.0), [f(p.x, 0.0, -4.0, 4.0), f(p.y, 0.0, -4.0, 4.0), f(p.w, 1.0, 0.005, 8.0), f(p.h, 1.0, 0.005, 8.0), f(p.cx, 0.5, 0.0, 1.0), f(p.cy, 0.5, 0.0, 1.0), f(p.z, 1.0, 0.01, 100.0)], p.e))
        .collect();
    if list.is_empty() {
        return None;
    }
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (cwf, chf, swf, shf) = (cw as f64, ch as f64, sw.max(2) as f64, sh.max(2) as f64);
    // The source box in source pixels at each point (never wider or taller than the source).
    let boxes: Vec<(f64, f64, f64, f64)> = list.iter().map(|(_, v, _)| (v[2] * cwf, v[3] * chf, (v[2] * cwf / v[6]).min(swf), (v[3] * chf / v[6]).min(shf))).collect();
    // The fixed size: as large as the source box is shown at most (more would not be seen), so no
    // detail is lost on the way, but the scaled source never larger than MOVING_MAX_SIDE.
    let side = |shown: &dyn Fn(&(f64, f64, f64, f64)) -> f64, src_box: &dyn Fn(&(f64, f64, f64, f64)) -> f64, full: f64| {
        let want = boxes.iter().map(shown).fold(2.0, f64::max).min(full);
        let smallest = boxes.iter().map(src_box).fold(f64::INFINITY, f64::min).max(1.0);
        let cap = MOVING_MAX_SIDE * smallest / full;
        ((want.min(cap) / 2.0).floor() as u32 * 2).max(2)
    };
    let fw = side(&|b| b.0.min(b.2), &|b| b.2, swf);
    let fh = side(&|b| b.1.min(b.3), &|b| b.3, shf);
    Some(MovingPiece { pts: list, fw, fh })
}

/// A canvas side: made even and kept to 16..=4096 (`fallback` when not a number).
fn canvas_side(v: f64, fallback: u32) -> u32 {
    let v = if v.is_finite() { v.clamp(16.0, 4096.0) } else { fallback as f64 };
    (v as u32) & !1
}

/// "#rrggbb" (or "#rgb") as R, G, B.
fn hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.trim().strip_prefix('#')?;
    if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = |a: usize, b: usize| u8::from_str_radix(&h[a..b], 16).ok();
    match h.len() {
        6 => Some((n(0, 2)?, n(2, 4)?, n(4, 6)?)),
        3 => Some((n(0, 1)? * 17, n(1, 2)? * 17, n(2, 3)? * 17)),
        _ => None,
    }
}

/// `c` on a source picture `sw` x `sh` pixels: sizes clamped, numbers that
/// are not numbers dropped, each piece clipped to the canvas (the source
/// part under the visible part is cropped, so nothing is ever scaled to more
/// than the canvas's size), pieces that do not show left out. A piece with a
/// track in `tracks` (same index) moves instead (keyframes).
fn canvas_plan(c: &Canvas, tracks: &[Vec<PiecePoint>], (sw, sh): (u32, u32)) -> CanvasPlan {
    let (cw, ch) = (canvas_side(c.width, 1080), canvas_side(c.height, 1920));
    let color = match c.background.trim() {
        "blur" => None,
        other => Some(hex_color(other).unwrap_or((0, 0, 0))),
    };
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let even = |v: f64| ((v / 2.0).round().max(0.0) as u32) * 2;
    let (sw_f, sh_f, cw_f, ch_f) = (sw as f64, sh as f64, cw as f64, ch as f64);
    let mut pieces = Vec::new();
    let mut moving = Vec::new();
    for (i, p) in c.pieces.iter().enumerate() {
        if let Some(mp) = tracks.get(i).and_then(|t| moving_piece(t, (cw, ch), (sw, sh))) {
            moving.push((pieces.len(), mp));
            continue;
        }
        let sx = fin(p.sx).clamp(0.0, 1.0);
        let sy = fin(p.sy).clamp(0.0, 1.0);
        let pw = fin(p.sw).min(1.0 - sx);
        let ph = fin(p.sh).min(1.0 - sy);
        let x = fin(p.x).clamp(-4.0, 4.0);
        let y = fin(p.y).clamp(-4.0, 4.0);
        let w = fin(p.w).min(8.0);
        let h = fin(p.h).min(8.0);
        if pw <= 0.0 || ph <= 0.0 || w <= 0.0 || h <= 0.0 {
            continue;
        }
        // The box on the canvas in pixels, and its visible part.
        let (dx, dy, dw, dh) = (x * cw_f, y * ch_f, w * cw_f, h * ch_f);
        let (vx0, vx1) = (even(dx.max(0.0)).min(cw), even((dx + dw).min(cw_f)).min(cw));
        let (vy0, vy1) = (even(dy.max(0.0)).min(ch), even((dy + dh).min(ch_f)).min(ch));
        if vx1 <= vx0 || vy1 <= vy0 {
            continue;
        }
        // The part of the source box shown there.
        let (fx, fy, fw, fh) = (sx * sw_f, sy * sh_f, pw * sw_f, ph * sh_f);
        let ux0 = fx + (vx0 as f64 - dx) / dw * fw;
        let ux1 = fx + (vx1 as f64 - dx) / dw * fw;
        let uy0 = fy + (vy0 as f64 - dy) / dh * fh;
        let uy1 = fy + (vy1 as f64 - dy) / dh * fh;
        let cx = even(ux0).min(sw.saturating_sub(2));
        let cy = even(uy0).min(sh.saturating_sub(2));
        let crop_w = even(ux1 - ux0).clamp(2, (sw - cx).max(2));
        let crop_h = even(uy1 - uy0).clamp(2, (sh - cy).max(2));
        pieces.push(([cx, cy, crop_w, crop_h], [vx0, vy0, vx1 - vx0, vy1 - vy0]));
    }
    CanvasPlan { width: cw, height: ch, color, pieces, moving }
}

/// A moving piece from `[{input}]` (the source, `sw` x `sh`) laid on
/// `[{cur}]` (the canvas, `cw` x `ch`) as `[{out}]`. `crop` takes a new
/// position every frame but its size only once, and keeps the size of the
/// first frame it saw, so: the source is scaled every frame (`eval=frame`)
/// to make its source box the FIXED size `fw` x `fh`; the box is cut out by
/// laying that on a frame of the fixed size at minus the box's position
/// (`overlay` follows every frame's size); the cut is scaled every frame to
/// the place's size and laid at the place. Width and height are scaled by
/// the same zoom at every instant, so the piece is never stretched.
fn moving_piece_chain(mp: &MovingPiece, input: &str, cur: &str, out: &str, i: usize, (cw, ch): (u32, u32), (sw, sh): (u32, u32)) -> String {
    let e = |k: usize| track_expr(&mp.pts.iter().map(|(t, v, ea)| (*t, v[k], *ea)).collect::<Vec<_>>());
    let (x, y, w, h, cx, cy, z) = (e(0), e(1), e(2), e(3), e(4), e(5), e(6));
    let (fw, fh) = (mp.fw, mp.fh);
    // Place size in canvas pixels, source box size in source pixels (never more than the source).
    let (pw, ph) = (format!("{cw}*({w})"), format!("{ch}*({h})"));
    let (bw, bh) = (format!("min({pw}/({z}),{sw})"), format!("min({ph}/({z}),{sh})"));
    format!(
        "[{input}]split=2[km{i}a][km{i}b];[km{i}a]crop={fw}:{fh}:0:0,setsar=1[km{i}c];\
         [km{i}b]scale=w='max({fw},{sw}*{fw}/{bw})':h='max({fh},{sh}*{fh}/{bh})':eval=frame,setsar=1[km{i}d];\
         [km{i}c][km{i}d]overlay=x='-clip(({cx})*overlay_w-{fw}/2,0,overlay_w-{fw})':y='-clip(({cy})*overlay_h-{fh}/2,0,overlay_h-{fh})'[km{i}e];\
         [km{i}e]scale=w='max(2,{pw})':h='max(2,{ph})':eval=frame,setsar=1[kq{i}];\
         [{cur}][kq{i}]overlay=x='{cw}*({x})':y='{ch}*({y})'[{out}]"
    )
}

/// Filter chains from `[{input}]` (the joined result, yuv420p, source size)
/// to `[{out}]` (the canvas). The background is made from the same frames
/// (split), so every frame keeps its timestamp: a flat color is a 2x2 crop
/// painted with `lutyuv` and scaled up; "blur" is the picture scaled to cover
/// a quarter-size canvas, Gaussian-blurred (sigma 2% of the canvas height),
/// darkened to 70% and scaled up. Each piece is a crop scaled to its box,
/// laid on in order. `matrix` / `range` are the source's (for the color).
fn canvas_chain(p: &CanvasPlan, input: &str, out: &str, (sw, sh): (u32, u32), matrix: &str, range: &str) -> Vec<String> {
    let (cw, ch) = (p.width, p.height);
    let n = p.pieces.len() + p.moving.len();
    let mut g = Vec::new();
    let bg_in = if n == 0 {
        input.to_string()
    } else {
        // Still pieces take cp0.., moving ones cpm0.. (so a canvas without moving pieces is built as before).
        let outs: String = std::iter::once("[cb]".to_string()).chain((0..p.pieces.len()).map(|i| format!("[cp{i}]"))).chain((0..p.moving.len()).map(|i| format!("[cpm{i}]"))).collect();
        g.push(format!("[{input}]split={}{outs}", n + 1));
        "cb".to_string()
    };
    let black = if range == "pc" { 0.0 } else { 16.0 };
    let bg = match p.color {
        Some((r, gg, b)) => {
            let (kr, kb) = if matrix == "bt601" { (0.299, 0.114) } else { (0.2126, 0.0722) };
            let (r, gg, b) = (r as f64 / 255.0, gg as f64 / 255.0, b as f64 / 255.0);
            let y = kr * r + (1.0 - kr - kb) * gg + kb * b;
            let (u, v) = ((b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr)));
            let (ys, cs) = if range == "pc" { (255.0, 255.0) } else { (219.0, 224.0) };
            let q = |x: f64| x.round().clamp(0.0, 255.0) as u8;
            let (y, u, v) = (q(black + ys * y), q(128.0 + cs * u), q(128.0 + cs * v));
            format!("crop=2:2:0:0,lutyuv=y={y}:u={u}:v={v},scale={cw}:{ch}:flags=neighbor,setsar=1")
        }
        None => {
            let (qw, qh) = (((cw / 4) & !1).max(2), ((ch / 4) & !1).max(2));
            // Cover: the smallest even size at least qw x qh with the source's shape.
            let k = (qw as f64 / sw.max(1) as f64).max(qh as f64 / sh.max(1) as f64);
            let up = |v: f64, min: u32| (((v / 2.0).ceil() as u32) * 2).max(min);
            let (aw, ah) = (up(sw as f64 * k, qw), up(sh as f64 * k, qh));
            let sigma = (0.02 * ch as f64 / 4.0).max(0.5);
            let dark = |c: f64| format!("'{c}+(val-{c})*0.7'");
            format!(
                "scale={aw}:{ah},crop={qw}:{qh},gblur=sigma={sigma:.3},lutyuv=y={}:u={}:v={},scale={cw}:{ch},setsar=1",
                dark(black),
                dark(128.0),
                dark(128.0)
            )
        }
    };
    let mut cur = "cbg".to_string();
    g.push(format!("[{bg_in}]{bg}[{cur}]"));
    let mut moved = 0;
    for slot in 0..=p.pieces.len() {
        // Moving pieces that come before this still one in the canvas's order.
        while let Some((_, mp)) = p.moving.get(moved).filter(|(at, _)| *at == slot) {
            let next = format!("cvm{moved}");
            g.push(moving_piece_chain(mp, &format!("cpm{moved}"), &cur, &next, moved, (cw, ch), (sw, sh)));
            cur = next;
            moved += 1;
        }
        let Some(([cx, cy, crw, crh], [vx, vy, vw, vh])) = p.pieces.get(slot) else { break };
        let i = slot;
        g.push(format!("[cp{i}]crop={crw}:{crh}:{cx}:{cy},scale={vw}:{vh},setsar=1[cq{i}];[{cur}][cq{i}]overlay={vx}:{vy}[cv{i}]"));
        cur = format!("cv{i}");
    }
    g.push(format!("[{cur}]format=yuv420p,setsar=1[{out}]"));
    g
}

/// Write a new clip beside `src` with `opts` applied, and return its library entry.
/// `progress` receives 0..100 while ffmpeg runs.
pub fn export(s: &Settings, src: &ClipMeta, opts: &ExportOpts, progress: &mut dyn FnMut(f32)) -> Result<ClipMeta, String> {
    // The overlay PNGs are temp files made for this export: gone when it ends, whatever happens.
    let _pngs = OverlayFiles(opts.overlays.iter().map(|o| PathBuf::from(&o.png)).collect());
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let src_path = PathBuf::from(&src.path);
    if !src_path.is_file() {
        return Err("The clip file is missing".into());
    }
    let duration = if src.duration > 0.0 { src.duration } else { library::probe(&ffmpeg, &src_path, false).map(|p| p.duration).unwrap_or(0.0) };
    if duration <= 0.0 {
        return Err("Could not read the clip's length".into());
    }
    let speed = if opts.speed.is_finite() { opts.speed.clamp(0.25, 4.0) } else { 1.0 };
    let pieces = kept_ranges(opts, duration);
    let ranges: Vec<(f64, f64)> = pieces.iter().map(|k| (k.a, k.b)).collect();
    let kept: f64 = ranges.iter().map(|(a, b)| b - a).sum();
    let Some(&(_, last_end)) = ranges.last() else {
        return Err("The selection is too short".into());
    };
    if kept < 0.1 {
        return Err("The selection is too short".into());
    }
    // The source span the ranges come from, `base`..`top`: the clip's audio
    // and the mix are laid out on it. In source order (not `ordered`) that is
    // the first range's start to the last one's end.
    let base = ranges.iter().map(|r| r.0).fold(f64::INFINITY, f64::min);
    let top = ranges.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
    let reaches_end = |b: f64| b >= duration - 0.002;
    // A range runs to the end of the file: no `-t` on the audio, so nothing is
    // lost to a duration that was rounded on the way through the UI.
    let to_end = reaches_end(top);
    let total = top - base;
    let k_count = ranges.len();

    // Nothing but a cut: the lossless trim is faster and keeps the original
    // quality. Its result stays kind "trim" and is named "(trim)", because
    // that is exactly what it is. The one range's own look decides (it
    // replaces `opts.color`), so a colored or blank range is never copied.
    let plain = pieces.iter().all(|k| !k.blank && is_neutral(&k.color));
    // Overlays, blur boxes and fades (on the result's own timeline, `out_total` long).
    let out_total = kept / speed;
    let layers = layer_plan(opts, out_total)?;
    if k_count == 1 && same(speed, 1.0) && plain && opts.tracks.is_empty() && opts.audio.is_empty() && layers.is_none() && opts.canvas.is_none() {
        let meta = crate::clip::trim(s, src, base, if to_end { duration } else { last_end }, true)?;
        progress(100.0);
        return Ok(meta);
    }

    let titles = audio_titles(&ffmpeg, &src_path)?;
    let mut tempo = atempo_chain(speed);
    // Every file the mix needs is checked here, before the long encode starts.
    let mut plan = audio_plan(&ffmpeg, &src_path, opts, &titles, duration, base, total, k_count, speed, kept)?;
    if let Some(l) = &layers {
        // The sound fades with the picture (after the speed change, so on the result's time).
        if l.fade_in > 0.0 {
            tempo += &format!(",afade=t=in:st=0:d={:.4}", l.fade_in);
        }
        if l.fade_out > 0.0 {
            tempo += &format!(",afade=t=out:st={:.4}:d={:.4}", (out_total - l.fade_out).max(0.0), l.fade_out);
        }
        if (l.fade_in > 0.0 || l.fade_out > 0.0) && matches!(plan, AudioPlan::Copy) {
            plan = AudioPlan::Tracks; // a copied track cannot fade
        }
    }

    // Video: one input per kept range, each seeked (cheap: a keyframe every
    // second) to the frame that was ON SCREEN at the range's start, i.e. the
    // last frame at or before it. On a variable-rate clip that frame can be
    // seconds earlier (a still screen sends no frames); it is pinned to the
    // range's first instant so a join never opens on black or on the previous
    // range's last picture. Each range is then placed at its position in the
    // result and `interleave` joins them by timestamp, which, unlike `concat`,
    // keeps every range exactly as long as its audio (concat guesses a VFR
    // segment's length from its mean frame interval).
    //
    // Look: when every range looks the same, the color is applied once after
    // the join, as before. Otherwise each range gets its own color (or is
    // painted black) on its own branch, before the join, and every branch ends
    // in the same pixel format and color tags so `interleave` joins like
    // with like. A blank range keeps its source frames and only overwrites
    // their pixels, so its size, frame timing and length are the source's.
    let (matrix, range) = video_colorspace(&ffmpeg, &src_path);
    let uniform = pieces.iter().all(|k| !k.blank && same_color(&k.color, &pieces[0].color));
    let tags = format!("setparams=colorspace={}:range={range}", if matrix == "bt601" { "smpte170m" } else { matrix });
    let black = if range == "pc" { 0 } else { 16 };
    let on_screen = frames_on_screen(&ffmpeg, &src_path, &ranges.iter().map(|r| r.0).collect::<Vec<_>>());
    let mut video_inputs: Vec<(f64, Option<f64>)> = Vec::new();
    let mut graph: Vec<String> = Vec::new();
    let mut placed = 0.0;
    for (k, &(a, b)) in ranges.iter().enumerate() {
        // Half a millisecond early so rounding never drops the frame itself;
        // frames are at least 4 ms apart even at 240 fps.
        let seek = on_screen[k].map(|f| (f - 0.0005).max(0.0)).filter(|f| *f <= a).unwrap_or(a);
        // Only the last range in the result may read on to the file's end:
        // anything past an earlier range's end would land in the next one.
        let open_end = k + 1 == k_count && reaches_end(b);
        // `-t` only bounds the reading; the exact end is the `trim`, because an
        // input's `-t` counts from its first frame, not from the seek point.
        video_inputs.push((seek, (!open_end).then(|| b - seek + 0.5)));
        // The end is half a millisecond early too, the mirror of the seek: a
        // frame at (or within rounding of) `b` is left to the next range,
        // which takes it as the frame on screen at its start. Touching ranges
        // then share no frame and lose none, whatever the timestamp rounding.
        let trim = if open_end { String::new() } else { format!("trim=end={:.6},", b - seek - 0.0005) };
        let label = if k_count == 1 { "vj".to_string() } else { format!("v{k}") };
        let look = if uniform {
            String::new()
        } else if pieces[k].blank {
            format!(",format=yuv420p,lutyuv=y={black}:u=128:v=128,{tags}")
        } else {
            let c = color_filter(&pieces[k].color, matrix, range).map(|f| format!(",{f}")).unwrap_or_default();
            format!("{c},format=yuv420p,{tags}")
        };
        graph.push(format!("[{k}:v:0]{trim}setpts='({placed:.6}+if(eq(N,0),0,max(0,T-{:.6})))/{speed:.6}/TB'{look}[{label}]", a - seek));
        placed += b - a;
    }
    if k_count > 1 {
        let labels: String = (0..k_count).map(|k| format!("[v{k}]")).collect();
        graph.push(format!("{labels}interleave=nb_inputs={k_count}[vj]"));
    }

    // After the join: speed cap, then color, ending in a pixel format every player takes.
    //
    // Output frame rate: the source is variable-rate, so the export stays
    // variable-rate and never invents frames. It is capped at the capture rate
    // by a drop-only selector (the recorder's: a running target time, so the
    // average rate is exact and no kept frame is more than half an interval
    // off). A speed-up would otherwise turn 60 fps into 120-240 fps, which
    // costs bitrate and which most players and upload sites cannot show. A
    // slow-down keeps every original frame (0.5x of 60 fps plays at up to
    // 30 fps) rather than duplicating or interpolating. `-r` cannot do this:
    // ffmpeg 8 refuses `-r` together with `-fps_mode vfr`, and CFR would
    // duplicate frames through every still stretch.
    let cap = if src.fps > 0 { src.fps.min(240) } else { 60 };
    let mut video = "[vj]null".to_string();
    if speed > 1.0 + 1e-3 {
        video += &format!(",select='if(gte(t,ld(0)),st(0,max(ld(0),t-1/{cap})+1/{cap})*0+1,0)'");
    }
    if let Some(f) = color_filter(&pieces[0].color, matrix, range).filter(|_| uniform) {
        video += ",";
        video += &f;
    }
    // The picture's size: the source's, or the canvas's when there is one.
    let src_size = if src.width > 0 && src.height > 0 {
        Some((src.width, src.height))
    } else if layers.is_some() || opts.canvas.is_some() {
        Some(library::probe(&ffmpeg, &src_path, false).map(|p| (p.width, p.height)).filter(|(w, h)| *w > 0 && *h > 0).ok_or("Could not read the clip's picture size")?)
    } else {
        None
    };
    let canvas = match (&opts.canvas, src_size) {
        (Some(c), Some(size)) => Some(canvas_plan(c, &opts.canvas_tracks, size)),
        _ => None,
    };
    // With overlays, blur boxes or fades the picture goes on through `layer_chain` (below) to [v].
    let after = if layers.is_some() { "vb" } else { "v" };
    match (&canvas, src_size) {
        (Some(c), Some(size)) => {
            // The canvas stage: pieces of the picture on a picture of the canvas's size.
            video += ",format=yuv420p[vc]";
            graph.push(video);
            graph.extend(canvas_chain(c, "vc", after, size, matrix, range));
        }
        _ => {
            video += &format!(",format=yuv420p[{after}]");
            graph.push(video);
        }
    }
    let mut maps: Vec<String> = vec!["-map".into(), "[v]".into()];
    let mut audio_args: Vec<String> = Vec::new();
    let aac = |a: &mut Vec<String>| a.extend(["-c:a", "aac", "-b:a", "160k"].map(String::from));

    // Audio inputs come after the video ones: the clip once (seeked to the
    // first kept range, so audio before it is never decoded), then each file.
    let clip_audio = k_count;
    let needs_clip = match &plan {
        AudioPlan::Silent => false,
        AudioPlan::Copy | AudioPlan::Tracks => true,
        AudioPlan::Mix { rows, .. } => rows.iter().any(|r| matches!(r.source, Source::Track(..))),
    };
    let first_file = clip_audio + needs_clip as usize;
    // The kept ranges on the timeline of the clip's audio input (0 = `base`),
    // in result order (`cut_chain` takes them in any order). A range that
    // reaches the file's end reads to the input's end (it has no `-t` then).
    let local: Vec<(f64, Option<f64>)> = ranges.iter().map(|&(a, b)| (a - base, (!reaches_end(b)).then_some(b - base))).collect();
    let mut file_inputs: Vec<(String, f64, f64)> = Vec::new();

    match &plan {
        AudioPlan::Silent => audio_args.push("-an".into()),
        AudioPlan::Copy => {
            // Unchanged audio is copied: no generation loss, titles carried over.
            maps.extend(["-map".to_string(), format!("{clip_audio}:a")]);
            audio_args.extend(["-c:a", "copy"].map(String::from));
        }
        AudioPlan::Tracks => {
            for (i, t) in titles.iter().enumerate() {
                graph.push(cut_chain(&format!("[{clip_audio}:a:{i}]"), &local, &format!("t{i}"), &tempo, &format!("[a{i}]")));
                maps.extend(["-map".to_string(), format!("[a{i}]")]);
                let title = t.clone().unwrap_or_else(|| format!("Track {}", i + 1));
                audio_args.extend([format!("-metadata:s:a:{i}"), format!("title={title}")]);
                audio_args.extend([format!("-disposition:a:{i}"), if i == 0 { "default".into() } else { "0".into() }]);
            }
            aac(&mut audio_args);
        }
        AudioPlan::Mix { rows, title } => {
            // Every row is brought to 48 kHz stereo and laid out on the timeline
            // that starts at `base`; the mix is then cut like the video.
            let mut labels = String::new();
            let mut thru = String::new();
            for (j, row) in rows.iter().enumerate() {
                let to_stereo = |ch: u32| {
                    // A mono source plays on both speakers at full level (as in the
                    // preview); the default upmix would put it 3 dB lower.
                    let up = if ch == 1 { "pan=stereo|c0=c0|c1=c0," } else { "" };
                    format!("aresample={MIX_RATE},{up}aformat=sample_fmts=fltp:sample_rates={MIX_RATE}:channel_layouts=stereo")
                };
                let gain = gain_filter(row.volume, &row.regions);
                let chain = match &row.source {
                    Source::Track(i, ch) => format!("[{clip_audio}:a:{i}]{},{gain}[r{j}]", to_stereo(*ch)),
                    Source::File { path, seek, len, at, channels } => {
                        let input = first_file + file_inputs.len();
                        file_inputs.push((path.clone(), *seek, *len));
                        let samples = (at * MIX_RATE as f64).round() as u64;
                        let delay = if samples > 0 { format!(",adelay=delays={samples}S:all=1") } else { String::new() };
                        format!("[{input}:a:0]{},asetpts=PTS-STARTPTS{delay},{gain}[r{j}]", to_stereo(*channels))
                    }
                };
                graph.push(chain);
                // A file that plays through cuts joins after them (below); the others are cut with the video.
                if row.through { thru += &format!("[r{j}]") } else { labels += &format!("[r{j}]") }
            }
            let n_thru = rows.iter().filter(|r| r.through).count();
            // A silent bed as long as the kept span: the mix always covers the
            // whole result, even where only a short file row is heard.
            graph.push(format!("anullsrc=r={MIX_RATE}:cl=stereo,atrim=end={total:.6},aformat=sample_fmts=fltp[bed]"));
            // normalize=0: each row plays at exactly the volume the user set, as
            // in the preview, instead of amix dividing by the input count.
            graph.push(format!("{labels}[bed]amix=inputs={}:duration=longest:normalize=0,atrim=end={total:.6}[mix]", rows.len() - n_thru + 1));
            if n_thru == 0 {
                graph.push(cut_chain("[mix]", &local, "m", &tempo, "[a]"));
            } else {
                // The cut mix first, then the files that play through the cuts on top of it, on the
                // result's own timeline. `duration=first`: the result stays as long as the video.
                graph.push(cut_chain("[mix]", &local, "m", "anull", "[mc]"));
                graph.push(format!("[mc]{thru}amix=inputs={}:duration=first:normalize=0,{tempo}[a]", n_thru + 1));
            }
            maps.extend(["-map", "[a]"].map(String::from));
            audio_args.extend(["-metadata:s:a:0".to_string(), format!("title={title}"), "-disposition:a:0".into(), "default".into()]);
            aac(&mut audio_args);
        }
    }

    // The overlay pictures are the last inputs.
    let first_png = first_file + file_inputs.len();
    if let Some(l) = &layers {
        // On a canvas, overlay PNGs arrive at the canvas's size and blur boxes are fractions of it.
        let size = canvas.as_ref().map(|c| (c.width, c.height)).or(src_size).ok_or("Could not read the clip's picture size")?;
        graph.extend(layer_chain(l, first_png, size, out_total));
    }
    // (path, moves): a moving overlay's picture is looped, so its per-frame scale gets a frame for every size.
    let png_inputs: Vec<(String, bool)> = layers.as_ref().map(|l| l.overlays.iter().map(|o| (o.0.clone(), o.3.is_some())).collect()).unwrap_or_default();
    let loop_rate = cap.min(60);
    // Keyframe tracks can make the graph long; Windows cuts a command line at 32767 characters, so a
    // long graph goes to a file (`-/filter_complex`, ffmpeg 7+). Short ones stay on the line as before.
    let graph_text = graph.join(";");
    let mut _graph_file = OverlayFiles(Vec::new());
    let graph_arg: [String; 2] = if graph_text.len() > 24_000 {
        let dir = overlay_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{}.txt", util::new_id()));
        fs::write(&path, &graph_text).map_err(|e| e.to_string())?;
        _graph_file.0.push(path.clone());
        ["-/filter_complex".into(), path.to_string_lossy().into_owned()]
    } else {
        ["-filter_complex".into(), graph_text]
    };

    let title = format!("{} (edit)", src.title);
    // Beside the source; from a draft, into its folder in the clips folder.
    let (dir, folder) = crate::clip::output_dir(s, src);
    let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
    let src_s = src_path.to_string_lossy().into_owned();

    let build = |enc: &str| -> Vec<String> {
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-nostats", "-y"].map(String::from).to_vec();
        for (seek, len) in &video_inputs {
            if k_count > 4 {
                // Every range has its own decoder, all open at once; fewer
                // threads each keeps memory in check on a 1440p clip.
                a.extend(["-threads", "4"].map(String::from));
            }
            a.extend(["-ss".to_string(), format!("{seek:.6}")]);
            if let Some(l) = len {
                a.extend(["-t".to_string(), format!("{l:.6}")]);
            }
            a.extend(["-i".to_string(), src_s.clone()]);
        }
        if needs_clip {
            a.extend(["-ss".to_string(), format!("{base:.6}")]);
            if !to_end {
                a.extend(["-t".to_string(), format!("{total:.6}")]);
            }
            a.extend(["-i".to_string(), src_s.clone()]);
        }
        for (path, seek, len) in &file_inputs {
            a.extend(["-ss".to_string(), format!("{seek:.6}"), "-t".into(), format!("{len:.6}"), "-i".into(), path.clone()]);
        }
        for (png, moves) in &png_inputs {
            if *moves {
                // A frame `loop_rate` times a second for the whole result (the overlay ends with the picture: `shortest`).
                a.extend(["-loop".to_string(), "1".into(), "-framerate".into(), loop_rate.to_string(), "-t".into(), format!("{:.3}", out_total + 1.0)]);
            }
            // One still picture each; `pattern_type none` so no character of the path is read as a frame number pattern.
            a.extend(["-f", "image2", "-pattern_type", "none", "-i"].map(String::from));
            a.push(png.clone());
        }
        a.extend(graph_arg.iter().cloned());
        a.extend(maps.iter().cloned());
        a.extend(video_codec_args(enc));
        // Frames keep their own timestamps (no CFR duplication); vfr only drops exact repeats.
        a.extend(["-fps_mode", "vfr"].map(String::from));
        // A keyframe each second of output, like the recorder, so the result can be trimmed losslessly too.
        a.extend(["-force_key_frames", "expr:gte(t,n_forced*1)"].map(String::from));
        a.extend(audio_args.iter().cloned());
        a.extend(["-metadata".to_string(), format!("title={title}"), "-movflags".into(), "+faststart".into(), "-progress".into(), "pipe:1".into()]);
        a.push(out.to_string_lossy().into_owned());
        a
    };

    let expected = kept / speed;
    let enc = pick_encoder(&ffmpeg, &s.encoder);
    let mut result = run_with_progress(&ffmpeg, &build(&enc), expected, progress);
    if let Err(e) = &result {
        if enc != "x264" {
            // A hardware encoder can be busy (the recorder and a game may hold its sessions); the CPU always works.
            util::log(&format!("export: {enc} failed ({e}), retrying with libx264"));
            let _ = fs::remove_file(&out);
            result = run_with_progress(&ffmpeg, &build("x264"), expected, progress);
        }
    }
    if let Err(e) = result {
        let _ = fs::remove_file(&out);
        return Err(format!("Export failed: {e}"));
    }
    if !fs::metadata(&out).is_ok_and(|m| m.len() > 0) {
        let _ = fs::remove_file(&out);
        return Err("Export failed: ffmpeg wrote no file".into());
    }
    progress(100.0);

    let mut meta = library::register(&ffmpeg, &out, &title, &src.game, "edit");
    // The file's average rate reflects how busy the screen was; report the
    // highest rate the export can hold instead (see `cap` above).
    meta.fps = ((cap as f64) * speed.min(1.0)).round().max(1.0) as u32;
    // Written beside the source (a draft: in its folder), so it sits in that library folder.
    meta.folder = folder;
    Ok(meta)
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    path: PathBuf,
    failed: u32,
}

impl Report {
    fn line(&mut self, text: &str) {
        use std::io::Write;
        // Appended line by line so a crash part-way still leaves the results so far.
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{text}");
        }
    }

    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if !ok {
            self.failed += 1;
        }
        let tag = if ok { "PASS" } else { "FAIL" };
        if detail.is_empty() {
            self.line(&format!("{tag} {name}"));
        } else {
            self.line(&format!("{tag} {name} - {detail}"));
        }
    }
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// Mean luma and mean saturation over every frame (ffmpeg `signalstats`).
fn signal_stats(ffmpeg: &Path, file: &Path) -> Option<(f64, f64)> {
    signal_stats_at(ffmpeg, file, 0.0, f64::INFINITY)
}

/// Like `signal_stats`, over the frames from `from` to `to` seconds only.
fn signal_stats_at(ffmpeg: &Path, file: &Path, from: f64, to: f64) -> Option<(f64, f64)> {
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(["-hide_banner", "-nostats", "-nostdin"]);
    if from > 0.0 {
        cmd.args(["-ss", &format!("{from:.3}")]);
    }
    if to.is_finite() {
        cmd.args(["-t", &format!("{:.3}", to - from)]);
    }
    let out = cmd
        .arg("-i")
        .arg(file)
        .args(["-an", "-vf", "signalstats,metadata=mode=print", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let (mut y, mut sat, mut n, mut m) = (0.0, 0.0, 0u32, 0u32);
    for line in text.lines() {
        if let Some(v) = line.split("lavfi.signalstats.YAVG=").nth(1).and_then(|v| v.trim().parse::<f64>().ok()) {
            y += v;
            n += 1;
        }
        if let Some(v) = line.split("lavfi.signalstats.SATAVG=").nth(1).and_then(|v| v.trim().parse::<f64>().ok()) {
            sat += v;
            m += 1;
        }
    }
    (n > 0 && m > 0).then(|| (y / n as f64, sat / m as f64))
}

/// Mean level in dB of audio stream `stream` after a narrow band-pass at `freq`.
fn band_db(ffmpeg: &Path, file: &Path, stream: u32, freq: u32) -> Option<f64> {
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostats", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{stream}"), "-af", &format!("bandpass=f={freq}:width_type=q:w=8,volumedetect"), "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse::<f64>().ok()))
}

fn run_ffmpeg(ffmpeg: &Path, args: &[&str]) -> Result<(), String> {
    let out = util::hidden_cmd(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(util::last_lines(&out.stderr, 2)) }
}

/// `glint.exe --editor-test <report.txt>`: headless checks of `audio_tracks` and `export`.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run_self_test(&mut r);
    let failed = r.failed;
    r.line(&format!("DONE failed={failed}"));
}

fn run_self_test(r: &mut Report) {
    let dir = util::data_dir().join("editor-test");
    let _ = fs::remove_dir_all(&dir);
    if let Err(e) = fs::create_dir_all(&dir) {
        r.check("create test folder", false, &e.to_string());
        return;
    }
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let mut s = Settings { clips_dir: dir.to_string_lossy().into_owned(), ..Settings::default() };
    s.encoder = "auto".into();
    let encoder = pick_encoder(&ffmpeg, &s.encoder);
    r.line(&format!("INFO encoder={encoder}"));
    // The log is in the test data folder; it tells whether exports fell back to libx264.
    let log_file = util::data_dir().join("glint.log");
    let _ = fs::remove_file(&log_file);

    // 8 s of moving video with a 2 s hole (3..5 s) like a still screen under
    // Graphics Capture, and three tones as Mix / Desktop / Microphone.
    let src_file = dir.join("source.mp4");
    let src_s = src_file.to_string_lossy().into_owned();
    let (f_mix, f_desk, f_mic) = (300u32, 1000u32, 3000u32);
    let sine = |f: u32| format!("sine=frequency={f}:sample_rate=48000:duration=8");
    let (s_mix, s_desk, s_mic) = (sine(f_mix), sine(f_desk), sine(f_mic));
    let built = run_ffmpeg(
        &ffmpeg,
        &[
            "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=8",
            "-f", "lavfi", "-i", &s_mix, "-f", "lavfi", "-i", &s_desk, "-f", "lavfi", "-i", &s_mic,
            "-filter_complex", "[0:v]select='not(between(t,3,5))'[v]",
            "-map", "[v]", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-fps_mode", "vfr",
            "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-force_key_frames", "expr:gte(t,n_forced*1)",
            "-c:a", "aac", "-b:a", "160k", "-ac", "2",
            "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop", "-metadata:s:a:2", "title=Microphone",
            "-movflags", "+faststart", &src_s,
        ],
    );
    r.check("build test clip", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    if built.is_err() {
        return;
    }
    let mut src = library::register(&ffmpeg, &src_file, "Test", "Test Game", "clip");
    src.fps = 30;
    src.folder = "Test Game".into();
    let src_probe = library::probe(&ffmpeg, &src_file, true).unwrap_or_default();
    r.line(&format!("INFO source duration={:.3} frames={} audio={}", src_probe.duration, src_probe.frames, src_probe.audio_streams));

    // --- audio_tracks
    let stale = util::tmp_dir().join("edit-stale-test");
    let recent = util::tmp_dir().join("edit-recent-test");
    let _ = fs::create_dir_all(&stale);
    let _ = fs::create_dir_all(&recent);
    let aged = age_dir(&stale, Duration::from_secs(2 * 86_400));
    let started = std::time::Instant::now();
    match audio_tracks(&s, &src) {
        Ok(tracks) => {
            let ms = started.elapsed().as_millis();
            let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
            r.check("audio_tracks returns 3 tracks", tracks.len() == 3, &format!("{} tracks in {ms} ms", tracks.len()));
            r.check("audio_tracks titles", titles == ["Mix", "Desktop", "Microphone"], &format!("{titles:?}"));
            let sizes: Vec<u64> = tracks.iter().map(|t| fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0)).collect();
            r.check("audio_tracks files non-empty", sizes.len() == 3 && sizes.iter().all(|&n| n > 0), &format!("{sizes:?}"));
            let lens: Vec<f64> = tracks.iter().map(|t| library::probe(&ffmpeg, Path::new(&t.path), false).map(|p| p.duration).unwrap_or(0.0)).collect();
            r.check("audio_tracks full length", lens.iter().all(|&d| near(d, 8.0, 0.2)), &format!("{lens:.2?}"));
            let mic_ok = tracks.get(2).and_then(|t| Some((band_db(&ffmpeg, Path::new(&t.path), 0, f_mic)?, band_db(&ffmpeg, Path::new(&t.path), 0, f_desk)?)));
            r.check("audio_tracks track 2 is the microphone tone", mic_ok.is_some_and(|(m, d)| m - d > 20.0), &format!("{mic_ok:?}"));
            let before: Vec<_> = tracks.iter().map(|t| fs::metadata(&t.path).and_then(|m| m.modified()).ok()).collect();
            let started = std::time::Instant::now();
            let again = audio_tracks(&s, &src);
            let after: Vec<_> = again.as_ref().map(|v| v.iter().map(|t| fs::metadata(&t.path).and_then(|m| m.modified()).ok()).collect()).unwrap_or_default();
            r.check("audio_tracks reuses cached files", again.is_ok() && before == after, &format!("{} ms", started.elapsed().as_millis()));
        }
        Err(e) => r.check("audio_tracks", false, &e),
    }
    r.check("stale edit folders removed", aged.is_ok() && !stale.exists(), &format!("{aged:?}"));
    r.check("recent edit folders kept", recent.exists(), "");
    let _ = fs::remove_dir_all(&stale);
    let _ = fs::remove_dir_all(&recent);

    // --- export
    let mut prog_log: Vec<f32> = Vec::new();
    let ex = |opts: ExportOpts, settings: &Settings, log: &mut Vec<f32>| -> Result<(ClipMeta, library::Probe), String> {
        log.clear();
        let m = export(settings, &src, &opts, &mut |p| log.push(p))?;
        let p = library::probe(&ffmpeg, Path::new(&m.path), true).ok_or("probe failed")?;
        Ok((m, p))
    };
    let desc = |m: &ClipMeta, p: &library::Probe| format!("kind={} dur={:.3} frames={} audio={} fps={} folder={}", m.kind, p.duration, p.frames, p.audio_streams, m.fps, m.folder);

    match ex(ExportOpts { start: 2.0, end: 6.0, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => r.check(
            "trim-only export uses the lossless cut",
            m.kind == "trim" && p.duration > 3.0 && p.duration < 5.5 && p.audio_streams == 3 && m.folder == "Test Game",
            &desc(&m, &p),
        ),
        Err(e) => r.check("trim-only export uses the lossless cut", false, &e),
    }

    match ex(ExportOpts { speed: 2.0, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => {
            r.check("speed 2.0 halves the duration", near(p.duration, 4.0, 0.35) && m.kind == "edit", &desc(&m, &p));
            r.check("speed 2.0 keeps all audio tracks", p.audio_streams == 3, "");
            let t = audio_titles(&ffmpeg, Path::new(&m.path)).unwrap_or_default();
            r.check("speed 2.0 keeps track titles", t.iter().map(|x| x.as_deref().unwrap_or("")).collect::<Vec<_>>() == ["Mix", "Desktop", "Microphone"], &format!("{t:?}"));
            // Twice the source's frames would have been shown in half the time; the cap keeps it at 30 fps.
            r.check("speed 2.0 frame rate capped at capture rate", p.frames > 0 && (p.frames as f64) <= src_probe.frames as f64 * 0.5 + 6.0, &format!("{} frames from {}", p.frames, src_probe.frames));
            let pitch = (band_db(&ffmpeg, Path::new(&m.path), 2, f_mic), band_db(&ffmpeg, Path::new(&m.path), 2, f_mic * 2));
            r.check("speed 2.0 keeps the pitch", matches!(pitch, (Some(a), Some(b)) if a - b > 20.0), &format!("{pitch:?}"));
            let mono = prog_log.windows(2).all(|w| w[1] >= w[0]);
            r.check("progress rises to 100", mono && prog_log.last() == Some(&100.0) && prog_log.len() >= 2, &format!("{} updates, last {:?}", prog_log.len(), prog_log.last()));
            r.check("edit stays in the source's library folder", m.folder == "Test Game" && Path::new(&m.path).parent() == src_file.parent(), &m.path);
        }
        Err(e) => r.check("speed 2.0 halves the duration", false, &e),
    }

    match ex(ExportOpts { speed: 0.5, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => {
            r.check("speed 0.5 doubles the duration", near(p.duration, 16.0, 0.5) && p.audio_streams == 3, &desc(&m, &p));
            r.check("speed 0.5 adds no frames", p.frames > 0 && p.frames <= src_probe.frames + 3, &format!("{} frames from {}", p.frames, src_probe.frames));
        }
        Err(e) => r.check("speed 0.5 doubles the duration", false, &e),
    }

    match ex(ExportOpts { start: 1.0, end: 7.0, speed: 2.0, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => r.check("trim 1..7 at 2.0x lasts 3 s", near(p.duration, 3.0, 0.35), &desc(&m, &p)),
        Err(e) => r.check("trim 1..7 at 2.0x lasts 3 s", false, &e),
    }

    let base = signal_stats(&ffmpeg, &src_file);
    r.line(&format!("INFO source YAVG/SATAVG {base:?}"));
    let bright = ExportOpts { color: Color { brightness: 1.4, ..Default::default() }, ..Default::default() };
    match ex(bright, &s, &mut prog_log).map(|(m, _)| signal_stats(&ffmpeg, Path::new(&m.path))) {
        Ok(st) => r.check("brightness 1.4 raises mean luma", matches!((base, st), (Some(b), Some(a)) if a.0 > b.0 + 8.0), &format!("{st:?}")),
        Err(e) => r.check("brightness 1.4 raises mean luma", false, &e),
    }
    let grey = ExportOpts { color: Color { saturation: 0.0, ..Default::default() }, ..Default::default() };
    match ex(grey, &s, &mut prog_log).map(|(m, _)| signal_stats(&ffmpeg, Path::new(&m.path))) {
        Ok(st) => r.check(
            "saturation 0 removes chroma",
            matches!((base, st), (Some(b), Some(a)) if b.1 > 10.0 && a.1 < 1.5 && near(a.0, b.0, 6.0)),
            &format!("{st:?}"),
        ),
        Err(e) => r.check("saturation 0 removes chroma", false, &e),
    }
    let mut cpu = s.clone();
    cpu.encoder = "x264".into();
    let warm = ExportOpts { color: Color { contrast: 1.3, gamma: 1.5, saturation: 1.6, ..Default::default() }, ..Default::default() };
    match ex(warm, &cpu, &mut prog_log) {
        Ok((m, p)) => r.check("libx264 export with all color steps", p.video_streams == 1 && near(p.duration, 8.0, 0.3), &desc(&m, &p)),
        Err(e) => r.check("libx264 export with all color steps", false, &e),
    }

    // Exact formula check on a flat mid-grey picture (Y only, so the matrix does not matter).
    color_formula_checks(r, &ffmpeg, &s, &dir);

    let mic_only = ExportOpts { tracks: vec![TrackVolume { index: 1, volume: 0.0 }, TrackVolume { index: 2, volume: 1.0 }], ..Default::default() };
    match ex(mic_only, &s, &mut prog_log) {
        Ok((m, p)) => {
            let lv = (band_db(&ffmpeg, Path::new(&m.path), 0, f_mic), band_db(&ffmpeg, Path::new(&m.path), 0, f_desk), band_db(&ffmpeg, Path::new(&m.path), 0, f_mix));
            let ok = p.audio_streams == 1 && matches!(lv, (Some(mic), Some(desk), Some(mix)) if mic - desk > 20.0 && mic - mix > 20.0);
            r.check("microphone only gives one track of the microphone", ok, &format!("{} / levels mic,desk,mix {lv:?}", desc(&m, &p)));
        }
        Err(e) => r.check("microphone only gives one track of the microphone", false, &e),
    }
    let both = ExportOpts { tracks: vec![TrackVolume { index: 1, volume: 1.0 }, TrackVolume { index: 2, volume: 0.5 }], ..Default::default() };
    match ex(both, &s, &mut prog_log) {
        Ok((m, p)) => {
            let lv = (band_db(&ffmpeg, Path::new(&m.path), 0, f_desk), band_db(&ffmpeg, Path::new(&m.path), 0, f_mic));
            // Mic at 0.5 sits 6 dB under the desktop: amix must not normalise.
            let ok = p.audio_streams == 1 && matches!(lv, (Some(d), Some(mi)) if near(d - mi, 6.0, 2.0) && d > -30.0);
            r.check("desktop 1.0 + microphone 0.5 mix at those volumes", ok, &format!("{} / levels desk,mic {lv:?}", desc(&m, &p)));
        }
        Err(e) => r.check("desktop 1.0 + microphone 0.5 mix at those volumes", false, &e),
    }
    let silent = ExportOpts { tracks: vec![TrackVolume { index: 1, volume: 0.0 }, TrackVolume { index: 2, volume: 0.0 }], ..Default::default() };
    match ex(silent, &s, &mut prog_log) {
        Ok((m, p)) => r.check("all volumes 0 gives no audio stream", p.audio_streams == 0 && p.video_streams == 1 && m.audio_tracks == 0, &desc(&m, &p)),
        Err(e) => r.check("all volumes 0 gives no audio stream", false, &e),
    }

    match ex(ExportOpts { speed: 4.0, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => r.check("speed 4.0 export", near(p.duration, 2.0, 0.3) && p.audio_streams == 3, &desc(&m, &p)),
        Err(e) => r.check("speed 4.0 export", false, &e),
    }
    match ex(ExportOpts { speed: 0.25, ..Default::default() }, &s, &mut prog_log) {
        Ok((m, p)) => r.check("speed 0.25 export", near(p.duration, 32.0, 0.8) && p.audio_streams == 3 && m.fps == 8, &desc(&m, &p)),
        Err(e) => r.check("speed 0.25 export", false, &e),
    }

    let count_before = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    let bad = export(&s, &src, &ExportOpts { start: 7.95, end: 8.0, speed: 2.0, ..Default::default() }, &mut |_| {});
    let count_after = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
    r.check("too-short selection is refused cleanly", bad.is_err() && count_before == count_after, &format!("{:?}", bad.err()));

    timeline_checks(r, &ffmpeg, &s, &src, &dir, (f_desk, f_mic), base);
    look_checks(r, &ffmpeg, &s, &src, f_desk);
    ordered_checks(r, &ffmpeg, &s, &dir);
    layer_checks(r, &ffmpeg, &s, &src, &src_file, (f_desk, 1));
    canvas_checks(r, &ffmpeg, &s, &src, &src_file, (f_desk, 1));
    keyframe_checks(r, &ffmpeg, &s, &src, &src_file);
    if let Some(clip) = std::env::var_os("GLINT_EDITOR_BENCH") {
        bench(r, &ffmpeg, &s, Path::new(&clip));
    }

    let fallbacks = fs::read_to_string(&log_file).unwrap_or_default().lines().filter(|l| l.contains("retrying with libx264")).count();
    r.check(&format!("{encoder} exports ran without falling back"), fallbacks == 0, &format!("{fallbacks} fallbacks"));
}

/// Standard base64 with padding (the test's way to hand a PNG to `save_overlay_png` as a data URL).
fn base64_encode(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

/// The first frame at or after `t` as `w`x`h` grey pixels.
fn grey_at(ffmpeg: &Path, file: &Path, t: f64, w: usize, h: usize) -> Option<Vec<u8>> {
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:{h},format=gray"), "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.stdout.len() == w * h).then_some(out.stdout)
}

/// Mean of a box (x0..x1, y0..y1) of a grey frame `w` pixels wide.
fn box_mean(f: &[u8], w: usize, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let mut n = 0.0;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += f[y * w + x] as f64;
            n += 1.0;
        }
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// Mean absolute difference of the same box in two grey frames (255 when either is missing).
fn box_diff(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>, w: usize, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let (Some(a), Some(b)) = (a, b) else { return 255.0 };
    let mut n = 0.0;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += (a[y * w + x] as f64 - b[y * w + x] as f64).abs();
            n += 1.0;
        }
    }
    if n > 0.0 { sum / n } else { 255.0 }
}

/// Fine detail in a box: mean absolute second difference along rows and columns
/// (a smooth gradient counts as none, so only edges and texture are measured).
fn box_detail(f: &Option<Vec<u8>>, w: usize, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> f64 {
    let Some(f) = f else { return 0.0 };
    let px = |x: usize, y: usize| f[y * w + x] as f64;
    let mut n = 0.0;
    let mut sum = 0.0;
    for y in y0.max(1)..y1.saturating_sub(1) {
        for x in x0.max(1)..x1.saturating_sub(1) {
            sum += (2.0 * px(x, y) - px(x - 1, y) - px(x + 1, y)).abs() + (2.0 * px(x, y) - px(x, y - 1) - px(x, y + 1)).abs();
            n += 1.0;
        }
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// Overlays, blur boxes and fades (0.1.25): each on the picture at the right
/// OUTPUT times only, also after cuts, a moved part and a speed change; the
/// sound fades with the picture; temp PNGs are removed; old options still parse.
/// The test clip (640x360, 8 s, still from 3 to 5 s) and its desktop tone
/// `f_desk` on audio stream `desk`.
fn layer_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, src_file: &Path, (f_desk, desk): (u32, u32)) {
    let ex = |opts: &ExportOpts| export(s, src, opts, &mut |_| {});
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    // A transparent picture with a white top-left quarter, as the editor would send it.
    let make_png = |name: &str| -> Option<PathBuf> {
        let p = odir.join(name);
        run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black@0.0:s=640x360,format=rgba,drawbox=x=0:y=0:w=320:h=180:color=white@1.0:t=fill:replace=1", "-frames:v", "1", &p.to_string_lossy()]).ok()?;
        p.is_file().then_some(p)
    };
    const W: usize = 64;
    const TL: (usize, usize, usize, usize) = (2, 2, 30, 16); // inside the white quarter
    const BR: (usize, usize, usize, usize) = (34, 20, 62, 34); // well away from it

    // Old options without the new fields, and new ones.
    let old: Result<ExportOpts, _> = serde_json::from_str(r#"{"start":1,"end":5,"speed":1,"color":{"brightness":1,"contrast":1,"saturation":1,"gamma":1},"tracks":[],"segments":[{"start":1,"end":5}],"audio":[],"ordered":false}"#);
    r.check("old export options without overlays still parse", old.as_ref().is_ok_and(|o| o.overlays.is_empty() && o.blurs.is_empty() && o.fade_in == 0.0 && o.fade_out == 0.0 && o.segments.len() == 1), &format!("{:?}", old.as_ref().err()));
    let new: Result<ExportOpts, _> = serde_json::from_str(r#"{"overlays":[{"png":"a.png","from":1,"to":2}],"blurs":[{"x":0.1,"y":0.2,"w":0.3,"h":0.4,"from":0,"to":3,"strength":0.03}],"fade_in":0.5,"fade_out":1}"#);
    r.check("export options with overlays, blurs and fades parse", new.as_ref().is_ok_and(|o| o.overlays.len() == 1 && o.blurs.len() == 1 && o.blurs[0].h == 0.4 && o.fade_in == 0.5 && o.fade_out == 1.0 && o.speed == 1.0), &format!("{:?}", new.as_ref().err()));

    // save_overlay_png: a data URL in, the same PNG bytes in a temp file out; anything else refused.
    let saved = make_png("roundtrip-src.png").and_then(|p| {
        let bytes = fs::read(&p).ok()?;
        let _ = fs::remove_file(&p);
        let path = save_overlay_png(&format!("data:image/png;base64,{}", base64_encode(&bytes))).ok()?;
        Some((fs::read(&path).ok()? == bytes, path))
    });
    let refused = save_overlay_png("data:image/png;base64,aGVsbG8gd29ybGQ=").is_err() && save_overlay_png("not base64 !!").is_err();
    r.check("overlay PNG data URL saved as the same file, other data refused", saved.as_ref().is_some_and(|(same, _)| *same) && refused, &format!("{saved:?} refused={refused}"));
    if let Some((_, p)) = saved {
        let _ = fs::remove_file(p);
    }

    // 1. An overlay from 1 to 2.5 s: on the picture inside, the source outside; never the lossless cut.
    let png = make_png("ov1.png");
    let opts = ExportOpts { overlays: vec![Overlay { png: png.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), from: 1.0, to: 2.5, ..Default::default() }], ..Default::default() };
    match ex(&opts) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (on, off) = (grey_at(ffmpeg, out, 1.5, W, 36), grey_at(ffmpeg, out, 6.0, W, 36));
            let (src_on, src_off) = (grey_at(ffmpeg, src_file, 1.5, W, 36), grey_at(ffmpeg, src_file, 6.0, W, 36));
            let white = on.as_ref().map(|f| box_mean(f, W, TL)).unwrap_or(0.0);
            let rest = box_diff(&on, &src_on, W, BR);
            let after = box_diff(&off, &src_off, W, TL);
            r.check("overlay is on the picture inside its time range", m.kind == "edit" && white > 200.0 && rest < 8.0, &format!("kind={} white quarter mean {white:.1}, rest differs by {rest:.1}", m.kind));
            r.check("overlay is not on the picture outside its time range", after < 8.0, &format!("quarter differs from the source by {after:.1} at 6 s"));
        }
        Err(e) => r.check("overlay is on the picture inside its time range", false, &e),
    }
    r.check("overlay PNG removed after the export", png.as_ref().is_some_and(|p| !p.exists()), &format!("{png:?}"));

    // 2. With a moved part and 2x speed: output 0..1 s is source 5.5..7.5, output 1..2 s is source 0.5..2.5.
    //    The overlay at OUTPUT 1.2..1.8 s shows over source 1.4..2.1 only.
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    let moved = ExportOpts { segments: vec![seg(5.5, 7.5), seg(0.5, 2.5)], ordered: true, speed: 2.0, ..Default::default() };
    let png2 = make_png("ov2.png");
    let with = ExportOpts { overlays: vec![Overlay { png: png2.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), from: 1.2, to: 1.8, ..Default::default() }], ..moved.clone() };
    match (ex(&moved), ex(&with)) {
        (Ok(a), Ok(b)) => {
            let (pa, pb) = (Path::new(&a.path), Path::new(&b.path));
            let inside = grey_at(ffmpeg, pb, 1.5, W, 36);
            let white = inside.as_ref().map(|f| box_mean(f, W, TL)).unwrap_or(0.0);
            let before = box_diff(&grey_at(ffmpeg, pa, 0.6, W, 36), &grey_at(ffmpeg, pb, 0.6, W, 36), W, TL);
            let later = box_diff(&grey_at(ffmpeg, pa, 1.9, W, 36), &grey_at(ffmpeg, pb, 1.9, W, 36), W, TL);
            let len = library::probe(ffmpeg, pb, false).map(|p| p.duration).unwrap_or(0.0);
            r.check("overlay after cuts, a moved part and 2x speed lands at its output time", white > 200.0 && before < 8.0 && later < 8.0 && near(len, 2.0, 0.25), &format!("white {white:.1} at 1.5 s, off by {before:.1} at 0.6 s and {later:.1} at 1.9 s, length {len:.2}"));
        }
        (a, b) => r.check("overlay after cuts, a moved part and 2x speed lands at its output time", false, &format!("{:?} / {:?}", a.err(), b.err())),
    }

    // 3. A blur box over the top-left quarter from 0 to 2.5 s: less detail inside, the rest as it was.
    let blur = ExportOpts { blurs: vec![BlurBox { x: 0.0, y: 0.0, w: 0.5, h: 0.5, from: 0.0, to: 2.5, strength: 0.03, ..Default::default() }], ..Default::default() };
    match ex(&blur) {
        Ok(m) => {
            let out = Path::new(&m.path);
            const BW: usize = 320;
            let (inbox, outbox) = ((8, 8, 152, 82), (176, 100, 312, 172));
            let (e1, s1) = (grey_at(ffmpeg, out, 1.5, BW, 180), grey_at(ffmpeg, src_file, 1.5, BW, 180));
            let (e6, s6) = (grey_at(ffmpeg, out, 6.0, BW, 180), grey_at(ffmpeg, src_file, 6.0, BW, 180));
            let (d_in, d_src) = (box_detail(&e1, BW, inbox), box_detail(&s1, BW, inbox));
            let (d_out, d_out_src) = (box_detail(&e1, BW, outbox), box_detail(&s1, BW, outbox));
            let (d_late, d_late_src) = (box_detail(&e6, BW, inbox), box_detail(&s6, BW, inbox));
            let ratio = |a: f64, b: f64| if b > 0.0 { a / b } else { 0.0 };
            r.check("blur box lowers the detail inside the box", d_src > 0.3 && ratio(d_in, d_src) < 0.4, &format!("detail {d_in:.2} vs source {d_src:.2}"));
            r.check("blur box leaves the rest of the picture", (0.75..1.33).contains(&ratio(d_out, d_out_src)), &format!("detail {d_out:.2} vs source {d_out_src:.2}"));
            r.check("blur box only inside its time range", (0.75..1.33).contains(&ratio(d_late, d_late_src)), &format!("detail at 6 s {d_late:.2} vs source {d_late_src:.2}"));
        }
        Err(e) => r.check("blur box lowers the detail inside the box", false, &e),
    }

    // 4. Fade in and out over 1 s each: first and last frames dark, the middle not; the sound follows.
    let fades = ExportOpts { fade_in: 1.0, fade_out: 1.0, ..Default::default() };
    match ex(&fades) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let y = |a: f64, b: f64| signal_stats_at(ffmpeg, out, a, b).map(|v| v.0).unwrap_or(-1.0);
            let (first, mid, last) = (y(0.0, 0.1), y(1.5, 2.5), y(7.88, 8.0));
            let streams = library::probe(ffmpeg, out, false).map(|p| p.audio_streams).unwrap_or(0);
            r.check("fade in and out: first and last frames dark, the middle not", m.kind == "edit" && first >= 0.0 && first < 30.0 && last >= 0.0 && last < 40.0 && mid > 50.0 && streams == 3, &format!("kind={} luma first {first:.1}, middle {mid:.1}, last {last:.1}, {streams} audio streams", m.kind));
            let db = |a: f64, b: f64| band_db_at(ffmpeg, out, desk, f_desk, a, b).unwrap_or(0.0);
            let (a0, a_mid, a_end) = (db(0.0, 0.2), db(2.0, 3.0), db(7.8, 7.98));
            r.check("fade in and out: the sound's level follows", a_mid > -40.0 && a_mid - a0 > 10.0 && a_mid - a_end > 10.0, &format!("desktop tone {a0:.1} / {a_mid:.1} / {a_end:.1} dB"));
        }
        Err(e) => r.check("fade in and out: first and last frames dark, the middle not", false, &e),
    }

    // 5. A failed export still removes its PNG; a missing PNG is refused before ffmpeg runs.
    let png3 = make_png("ov3.png");
    let missing_audio = AudioEdit { source: "file".into(), path: odir.join("no-such-file.wav").to_string_lossy().into_owned(), volume: 1.0, ..Default::default() };
    let bad = ex(&ExportOpts { overlays: vec![Overlay { png: png3.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), from: 0.0, to: 8.0, ..Default::default() }], audio: vec![missing_audio], ..Default::default() });
    r.check("overlay PNG removed when the export fails", bad.is_err() && png3.as_ref().is_some_and(|p| !p.exists()), &format!("{:?}", bad.err()));
    let gone = ex(&ExportOpts { overlays: vec![Overlay { png: odir.join("never-written.png").to_string_lossy().into_owned(), from: 0.0, to: 8.0, ..Default::default() }], ..Default::default() });
    r.check("missing overlay PNG is refused cleanly", gone.as_ref().is_err_and(|e| e.contains("overlay")), &format!("{:?}", gone.err()));
}

/// A box of the first frame at or after `t` (x, y, w, h: fractions of the
/// picture after the filter steps `pre`), scaled to `w` x `h` grey pixels.
fn grey_box_at(ffmpeg: &Path, file: &Path, t: f64, pre: &str, (bx, by, bw, bh): (f64, f64, f64, f64), w: usize, h: usize) -> Option<Vec<u8>> {
    let vf = format!("{pre}crop=iw*{bw:.6}:ih*{bh:.6}:iw*{bx:.6}:ih*{by:.6},scale={w}:{h},format=gray");
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", &vf, "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.stdout.len() == w * h).then_some(out.stdout)
}

/// Mean R, G, B of a box (fractions) of the frame at `t`, read as BT.709 limited range.
fn rgb_box_at(ffmpeg: &Path, file: &Path, t: f64, (bx, by, bw, bh): (f64, f64, f64, f64)) -> Option<(f64, f64, f64)> {
    let vf = format!("crop=iw*{bw:.6}:ih*{bh:.6}:iw*{bx:.6}:ih*{by:.6},scale=8:8:in_color_matrix=bt709:in_range=tv,format=rgb24");
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", &vf, "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.stdout.len() != 8 * 8 * 3 {
        return None;
    }
    let ch = |c: usize| out.stdout.iter().skip(c).step_by(3).map(|v| *v as f64).sum::<f64>() / 64.0;
    Some((ch(0), ch(1), ch(2)))
}

/// The canvas (0.1.26): size, pieces in their boxes (also over an edge and
/// overlapping), the three backgrounds, overlays and blur boxes on the canvas,
/// cuts / order / speed, and hostile numbers. The test clip is 640x360, 8 s,
/// still from 3 to 5 s, with the desktop tone `f_desk` on audio stream `desk`.
fn canvas_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, src_file: &Path, (f_desk, desk): (u32, u32)) {
    let ex = |opts: &ExportOpts| export(s, src, opts, &mut |_| {});
    let piece = |sx: f64, sy: f64, sw: f64, sh: f64, x: f64, y: f64, w: f64, h: f64| CanvasPiece { sx, sy, sw, sh, x, y, w, h };
    // A box shrunk by 6% of its size on every side, away from scaled edges.
    let inset = |(x, y, w, h): (f64, f64, f64, f64)| (x + w * 0.06, y + h * 0.06, w * 0.88, h * 0.88);
    let (gw, gh) = (40usize, 24usize);
    // Mean difference between a box of the export at `t` and a box of the source at `ts`.
    let same_as = |out: &Path, t: f64, ob: (f64, f64, f64, f64), ts: f64, sb: (f64, f64, f64, f64)| {
        box_diff(&grey_box_at(ffmpeg, out, t, "", inset(ob), gw, gh), &grey_box_at(ffmpeg, src_file, ts, "", inset(sb), gw, gh), gw, (0, 0, gw, gh))
    };
    let mean_of = |out: &Path, t: f64, b: (f64, f64, f64, f64)| grey_box_at(ffmpeg, out, t, "", b, 16, 8).map(|f| box_mean(&f, 16, (0, 0, 16, 8))).unwrap_or(-1.0);
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };

    // --- options
    let old: Result<ExportOpts, _> = serde_json::from_str(r#"{"start":1,"end":5,"speed":1,"overlays":[],"blurs":[],"fade_in":0,"fade_out":0}"#);
    let null: Result<ExportOpts, _> = serde_json::from_str(r#"{"start":1,"end":5,"canvas":null}"#);
    let full: Result<ExportOpts, _> = serde_json::from_str(r##"{"canvas":{"width":1080,"height":1920,"background":"#112233","pieces":[{"sx":0.1,"sy":0.2,"sw":0.3,"sh":0.4,"x":-0.5,"y":0.5,"w":2,"h":0.25}]}}"##);
    r.check(
        "export options without a canvas, with canvas null and with a canvas parse",
        old.as_ref().is_ok_and(|o| o.canvas.is_none() && o.start == 1.0)
            && null.as_ref().is_ok_and(|o| o.canvas.is_none() && o.end == 5.0)
            && full.as_ref().is_ok_and(|o| o.canvas.as_ref().is_some_and(|c| c.height == 1920.0 && c.background == "#112233" && c.pieces.len() == 1 && c.pieces[0].x == -0.5 && c.pieces[0].w == 2.0)),
        &format!("{:?} / {:?} / {:?}", old.err(), null.err(), full.err()),
    );

    // --- the plan: odd sizes made even, hostile numbers never reach ffmpeg
    let odd = canvas_plan(&Canvas { width: 1081.0, height: 1351.0, ..Default::default() }, &[], (640, 360));
    let hostile = Canvas {
        width: -5.0,
        height: 1e12,
        background: "url(x);drawtext".into(),
        pieces: vec![
            piece(f64::NAN, 0.0, f64::NAN, 0.5, 0.0, 0.0, 1.0, 1.0),
            piece(0.0, 0.0, 0.5, 0.5, 0.0, 0.0, -1.0, 1.0),
            piece(0.0, 0.0, 0.5, 0.5, 1e9, 0.0, 1.0, 1.0),
            piece(0.0, 0.0, 1e9, 1e9, 0.0, 0.0, 1.0, 1.0),
            piece(0.2, 0.2, 0.5, 0.5, -1e300, f64::INFINITY, 1e300, 1e300),
            piece(0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 1.0, 1.0),
        ],
    };
    let hp = canvas_plan(&hostile, &[], (640, 360));
    let ok_box = |&([cx, cy, cw, ch], [vx, vy, vw, vh]): &([u32; 4], [u32; 4])| {
        [cx, cy, cw, ch, vx, vy, vw, vh].iter().all(|v| v % 2 == 0) && cw > 0 && ch > 0 && vw > 0 && vh > 0 && cx + cw <= 640 && cy + ch <= 360 && vx + vw <= hp.width && vy + vh <= hp.height
    };
    let chain = canvas_chain(&hp, "vc", "v", (640, 360), "bt709", "tv").join(";");
    let clean = !chain.contains("NaN") && !chain.contains("inf") && !chain.contains("url") && !chain.contains('-');
    r.check(
        "canvas sizes made even and kept to 16..4096, hostile pieces dropped or clamped",
        (odd.width, odd.height) == (1080, 1350) && (hp.width, hp.height) == (16, 4096) && hp.color == Some((0, 0, 0)) && hp.pieces.len() == 2 && hp.pieces.iter().all(ok_box) && clean,
        &format!("odd {}x{}, hostile {}x{} {:?} {:?}", odd.width, odd.height, hp.width, hp.height, hp.color, hp.pieces),
    );

    // --- 1. A 1080x1920 canvas with three pieces: a strip from the top right,
    //        the middle hanging over both side edges, a strip from the bottom left; black around them.
    let a = (0.0, 0.05, 1.0, 0.158203125); // 256x72 source pixels shown 1080 wide
    let b = (-0.1, 0.25, 1.2, 0.50625); // 480x360 shown 1296 wide
    let c = (0.1, 0.8, 0.8, 0.16875); // 192x72 shown 864 wide
    let three = Canvas {
        width: 1080.0,
        height: 1920.0,
        background: "black".into(),
        pieces: vec![piece(0.6, 0.0, 0.4, 0.2, a.0, a.1, a.2, a.3), piece(0.125, 0.0, 0.75, 1.0, b.0, b.1, b.2, b.3), piece(0.0, 0.8, 0.3, 0.2, c.0, c.1, c.2, c.3)],
    };
    // The visible part of the middle piece, and the source under it.
    let b_out = (0.0, 0.25, 1.0, 0.50625);
    let b_src = (0.125 + 0.75 * (0.1 / 1.2), 0.0, 0.75 / 1.2, 1.0);
    let canvas_opts = ExportOpts { canvas: Some(three.clone()), ..Default::default() };
    let first = ex(&canvas_opts);
    match &first {
        Ok(m) => {
            let out = Path::new(&m.path);
            let p = library::probe(ffmpeg, out, true).unwrap_or_default();
            let tone = (band_db_at(ffmpeg, out, desk, f_desk, 1.0, 2.0), band_db_at(ffmpeg, src_file, desk, f_desk, 1.0, 2.0));
            r.check(
                "1080x1920 canvas export has that size, the full length and the audio",
                m.kind == "edit" && (m.width, m.height) == (1080, 1920) && (p.width, p.height) == (1080, 1920) && near(p.duration, 8.0, 0.15) && p.audio_streams == 3 && matches!(tone, (Some(x), Some(y)) if near(x, y, 1.5)),
                &format!("{}x{} (probe {}x{}) dur={:.3} audio={} tone {tone:?}", m.width, m.height, p.width, p.height, p.duration, p.audio_streams),
            );
            let (da, db, dc) = (same_as(out, 1.5, a, 1.5, (0.6, 0.0, 0.4, 0.2)), same_as(out, 1.5, b_out, 1.5, b_src), same_as(out, 1.5, c, 1.5, (0.0, 0.8, 0.3, 0.2)));
            let wrong = same_as(out, 1.5, a, 1.5, (0.0, 0.8, 0.3, 0.2));
            r.check("each canvas piece shows its part of the source in its box", da < 12.0 && db < 12.0 && dc < 12.0 && wrong > da + 10.0, &format!("diff top {da:.1}, middle (over the edges) {db:.1}, bottom {dc:.1}; wrong part {wrong:.1}"));
            let gaps = [(0.0, 0.0, 1.0, 0.045), (0.0, 0.212, 1.0, 0.035), (0.0, 0.76, 1.0, 0.035), (0.0, 0.972, 1.0, 0.028), (0.0, 0.81, 0.09, 0.15)];
            let levels: Vec<f64> = gaps.iter().map(|g| mean_of(out, 1.5, *g)).collect();
            r.check("black canvas is black outside the pieces", levels.iter().all(|v| (0.0..6.0).contains(v)), &format!("luma {levels:.1?}"));
        }
        Err(e) => r.check("1080x1920 canvas export has that size, the full length and the audio", false, e),
    }

    // --- 2. An odd size, a color, and two overlapping pieces (the later one on top); 0.5..2.5 s of the source.
    let two = Canvas {
        width: 1081.0,
        height: 1351.0,
        background: "#3366cc".into(),
        pieces: vec![piece(0.0, 0.0, 0.5, 0.5, 0.1, 0.1, 0.6, 0.27), piece(0.5, 0.5, 0.5, 0.5, 0.3, 0.2, 0.6, 0.27)],
    };
    match ex(&ExportOpts { segments: vec![seg(0.5, 2.5)], canvas: Some(two), ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            // Output 1.0 s is source 1.5 s. Where they overlap the second piece shows; elsewhere the first.
            let top = same_as(out, 1.0, (0.3, 0.2, 0.4, 0.17), 1.5, (0.5, 0.5, 0.5 * 0.4 / 0.6, 0.5 * 0.17 / 0.27));
            let under = same_as(out, 1.0, (0.3, 0.2, 0.4, 0.17), 1.5, (0.5 * 0.2 / 0.6, 0.5 * 0.1 / 0.27, 0.5 * 0.4 / 0.6, 0.5 * 0.17 / 0.27));
            let lower = same_as(out, 1.0, (0.1, 0.1, 0.2, 0.1), 1.5, (0.0, 0.0, 0.5 * 0.2 / 0.6, 0.5 * 0.1 / 0.27));
            r.check("odd canvas size made even (1081x1351 to 1080x1350)", (m.width, m.height) == (1080, 1350), &format!("{}x{}", m.width, m.height));
            r.check("overlapping pieces: the later one on top", top < 12.0 && lower < 12.0 && under > top + 10.0, &format!("overlap vs top piece {top:.1}, vs the one under {under:.1}; first piece alone {lower:.1}"));
            let rgb = rgb_box_at(ffmpeg, out, 1.0, (0.1, 0.75, 0.8, 0.2));
            r.check("#rrggbb canvas is that color outside the pieces", rgb.is_some_and(|(rr, g, bb)| near(rr, 51.0, 6.0) && near(g, 102.0, 6.0) && near(bb, 204.0, 6.0)), &format!("{rgb:?} for (51, 102, 204)"));
        }
        Err(e) => r.check("odd canvas size made even (1081x1351 to 1080x1350)", false, &e),
    }

    // --- 3. The blurred background: much less detail than the source there, darker, not black.
    let blur_bg = Canvas { width: 1080.0, height: 1920.0, background: "blur".into(), pieces: vec![piece(0.0, 0.0, 1.0, 1.0, 0.0, 0.35, 1.0, 0.31640625)] };
    match ex(&ExportOpts { segments: vec![seg(0.5, 2.5)], canvas: Some(blur_bg), ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let cover = "scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,";
            let (rw, rh) = (216usize, 120usize);
            let mut parts = Vec::new();
            let mut ok = true;
            for region in [(0.0, 0.02, 1.0, 0.3), (0.0, 0.7, 1.0, 0.28)] {
                let (o, refr) = (grey_box_at(ffmpeg, out, 1.0, "", region, rw, rh), grey_box_at(ffmpeg, src_file, 1.5, cover, region, rw, rh));
                let (d_o, d_r) = (box_detail(&o, rw, (0, 0, rw, rh)), box_detail(&refr, rw, (0, 0, rw, rh)));
                let (m_o, m_r) = (o.as_ref().map(|f| box_mean(f, rw, (0, 0, rw, rh))).unwrap_or(0.0), refr.as_ref().map(|f| box_mean(f, rw, (0, 0, rw, rh))).unwrap_or(0.0));
                // Grey pixels are full range, so 70% of the level is 70% of the grey value.
                let want = 0.7 * m_r;
                ok &= d_r > 0.5 && d_o < d_r * 0.25 && m_o > 25.0 && near(m_o, want, want * 0.2 + 4.0);
                parts.push(format!("detail {d_o:.2} vs {d_r:.2}, luma {m_o:.1} (70% of the source's {m_r:.1} is {want:.1})"));
            }
            r.check("blur canvas: blurred, darkened, not black", ok, &parts.join("; "));
        }
        Err(e) => r.check("blur canvas: blurred, darkened, not black", false, &e),
    }

    // --- 4. An overlay PNG at the canvas's size and a blur box (fractions of the canvas) on the canvas.
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    let png = odir.join("canvas-ov.png");
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black@0.0:s=1080x1920,format=rgba,drawbox=x=0:y=1100:w=540:h=300:color=white@1.0:t=fill:replace=1", "-frames:v", "1", &png.to_string_lossy()]);
    let layered = ExportOpts {
        overlays: vec![Overlay { png: png.to_string_lossy().into_owned(), from: 1.0, to: 2.5, ..Default::default() }],
        blurs: vec![BlurBox { x: 0.6, y: 0.52, w: 0.4, h: 0.2, from: 0.0, to: 8.0, strength: 0.03, ..Default::default() }],
        ..canvas_opts.clone()
    };
    match (made, &first, ex(&layered)) {
        (Ok(()), Ok(plain), Ok(m)) => {
            let (out, base) = (Path::new(&m.path), Path::new(&plain.path));
            let white = mean_of(out, 1.5, (0.05, 0.6, 0.4, 0.1));
            let (rw, rh) = (108usize, 96usize);
            let inbox = (0.62, 0.54, 0.36, 0.16);
            let (d_in, d_base) = (box_detail(&grey_box_at(ffmpeg, out, 1.5, "", inbox, rw, rh), rw, (0, 0, rw, rh)), box_detail(&grey_box_at(ffmpeg, base, 1.5, "", inbox, rw, rh), rw, (0, 0, rw, rh)));
            let rest = box_diff(&grey_box_at(ffmpeg, out, 1.5, "", (0.5, 0.3, 0.45, 0.15), gw, gh), &grey_box_at(ffmpeg, base, 1.5, "", (0.5, 0.3, 0.45, 0.15), gw, gh), gw, (0, 0, gw, gh));
            r.check("overlay PNG lands at its canvas position", white > 200.0, &format!("white box mean {white:.1}"));
            r.check("blur box lands at its canvas position", d_base > 0.3 && d_in < d_base * 0.4 && rest < 6.0, &format!("detail {d_in:.2} vs {d_base:.2} without; elsewhere off by {rest:.1}"));
        }
        (made, first, e) => r.check("overlay PNG lands at its canvas position", false, &format!("{made:?} / {:?} / {:?}", first.as_ref().err(), e.err())),
    }
    let _ = fs::remove_file(&png);

    // --- 5. With cuts, a moved part and 2x speed: output 0..1 s is source 5.5..7.5, 1..2 s is source 0.5..2.5.
    let moved = ExportOpts { segments: vec![seg(5.5, 7.5), seg(0.5, 2.5)], ordered: true, speed: 2.0, ..canvas_opts.clone() };
    match ex(&moved) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let len = library::probe(ffmpeg, out, false).map(|p| p.duration).unwrap_or(0.0);
            let (r1, w1) = (same_as(out, 0.6, b_out, 6.7, b_src), same_as(out, 0.6, b_out, 0.6, b_src));
            let (r2, w2) = (same_as(out, 1.5, b_out, 1.5, b_src), same_as(out, 1.5, b_out, 6.5, b_src));
            r.check(
                "canvas with cuts, a moved part and 2x speed: length and frames at their times",
                near(len, 2.0, 0.1) && (m.width, m.height) == (1080, 1920) && r1 < 12.0 && r2 < 12.0 && w1 > r1 + 5.0 && w2 > r2 + 5.0,
                &format!("length {len:.2}; at 0.6 s {r1:.1} (wrong time {w1:.1}), at 1.5 s {r2:.1} (wrong time {w2:.1})"),
            );
        }
        Err(e) => r.check("canvas with cuts, a moved part and 2x speed: length and frames at their times", false, &e),
    }

    // --- 6. Hostile numbers in a real export: a working file of a sane size.
    let wild = Canvas { width: f64::NAN, height: 1e12, ..hostile };
    match ex(&ExportOpts { segments: vec![seg(0.0, 1.0)], canvas: Some(wild), ..Default::default() }) {
        Ok(m) => {
            let p = library::probe(ffmpeg, Path::new(&m.path), false).unwrap_or_default();
            r.check("canvas with hostile numbers exports a working file", (p.width, p.height) == (1080, 4096) && near(p.duration, 1.0, 0.15), &format!("{}x{} dur={:.3}", p.width, p.height, p.duration));
        }
        Err(e) => r.check("canvas with hostile numbers exports a working file", !e.starts_with("Export failed") && !e.is_empty(), &format!("clean error: {e}")),
    }
}

/// The box (x0, y0, x1, y1, inclusive) of the bright pixels (> 200) of a
/// `w` x `h` grey frame, or None when there are none.
fn bright_box(f: &Option<Vec<u8>>, w: usize, h: usize) -> Option<(f64, f64, f64, f64)> {
    let f = f.as_ref()?;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if f[y * w + x] > 200 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    (x0 != usize::MAX).then_some((x0 as f64, y0 as f64, x1 as f64, y1 as f64))
}

/// Keyframes (0.1.27): a moving overlay at the right place and size at three
/// times, eased ("Smooth") against even, across a cut between two keyframes
/// and at 2x speed; a moving canvas piece showing the right part of the source
/// at the right place, unstretched, at three times; a moving blur box; a
/// track long enough that the graph goes through a file; the track maths.
/// The test clip is 640x360, 8 s, still from 3 to 5 s.
fn keyframe_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, src_file: &Path) {
    let ex = |opts: &ExportOpts| export(s, src, opts, &mut |_| {});
    let mv = |t: f64, dx: f64, dy: f64, sx: f64, sy: f64, e: bool| MovePoint { t, dx, dy, sx, sy, e };
    let blank = |a: f64, b: f64| Range { start: a, end: b, blank: true, ..Default::default() };
    const W: usize = 640;
    const H: usize = 360;
    let frame = |out: &Path, t: f64| grey_at(ffmpeg, out, t, W, H);
    // Where a box whose base is `b` (fractions) is with move (dx, dy, sx, sy): (x0, y0, x1, y1) pixels.
    let expect = |(bx, by, bw, bh): (f64, f64, f64, f64), (dx, dy, sx, sy): (f64, f64, f64, f64)| {
        let (cx, cy, w, h) = ((bx + bw / 2.0 + dx) * W as f64, (by + bh / 2.0 + dy) * H as f64, bw * sx * W as f64, bh * sy * H as f64);
        (cx - w / 2.0, cy - h / 2.0, cx + w / 2.0 - 1.0, cy + h / 2.0 - 1.0)
    };
    let off = |a: Option<(f64, f64, f64, f64)>, b: (f64, f64, f64, f64)| a.map(|a| (a.0 - b.0).abs().max((a.1 - b.1).abs()).max((a.2 - b.2).abs()).max((a.3 - b.3).abs())).unwrap_or(999.0);

    // --- options and maths
    let parsed: Result<ExportOpts, _> = serde_json::from_str(
        r#"{"overlays":[{"png":"a.png","from":0,"to":3,"x":0.1,"y":0.2,"w":0.3,"h":0.1,"track":[{"t":0,"dx":0.1,"dy":0,"sx":1,"sy":1},{"t":2,"dx":0.2,"dy":0.1,"sx":2,"sy":2,"e":true}]}],"blurs":[{"x":0.1,"y":0.1,"w":0.2,"h":0.2,"from":0,"to":3,"strength":0.03,"track":[{"t":1,"dx":0.5}]}],"canvas":{"width":1080,"height":1920,"background":"black","pieces":[{"sx":0,"sy":0,"sw":1,"sh":1,"x":0,"y":0,"w":1,"h":0.3}]},"canvas_tracks":[[{"t":0,"x":0,"y":0,"w":1,"h":0.3,"cx":0.5,"cy":0.5,"z":0.5}]]}"#,
    );
    r.check(
        "keyframe tracks parse; a missing scale is 1",
        parsed.as_ref().is_ok_and(|o| o.overlays[0].track.len() == 2 && o.overlays[0].track[1].e && o.overlays[0].w == 0.3 && o.blurs[0].track[0].sx == 1.0 && o.blurs[0].track[0].dx == 0.5 && o.canvas_tracks[0][0].z == 0.5 && o.canvas_tracks[0][0].cx == 0.5),
        &format!("{:?}", parsed.as_ref().err()),
    );
    let pts = [(0.5, 0.0, true), (2.5, 1.0, false), (2.5, 3.0, false), (4.0, 2.0, false)];
    let expr = track_expr(&pts);
    let vals: Vec<f64> = [0.0, 1.0, 1.5, 2.49, 2.5, 3.25, 9.0].iter().map(|t| track_value(&pts, *t)).collect();
    let want = [0.0, 0.15625, 0.5, 0.999925, 3.0, 2.5, 2.0];
    r.check(
        "track maths: held before and after, eased between keyframes, a jump at a cut",
        vals.iter().zip(want).all(|(a, b)| near(*a, b, 1e-4)) && expr.contains("gte(t,2.5)") && expr.contains("*(3-2*") && !expr.contains("if("),
        &format!("{vals:.5?} / {expr}"),
    );

    // A white 64x36 box (0.1 x 0.1 of the picture) as the UI sends a moving item: only its box.
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    let make_box = |name: &str| -> String {
        let p = odir.join(name);
        let _ = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=white:s=64x36,format=rgba", "-frames:v", "1", &p.to_string_lossy()]);
        p.to_string_lossy().into_owned()
    };
    let base = (0.05, 0.1, 0.1, 0.1);
    let moving = |png: String, from: f64, to: f64, track: Vec<MovePoint>| Overlay { png, from, to, x: base.0, y: base.1, w: base.2, h: base.3, track };

    // --- 1. A moving overlay on a black picture: still before its first keyframe, half way at the middle, at the end after the last.
    let lin = vec![mv(0.5, 0.0, 0.0, 1.0, 1.0, false), mv(2.5, 0.5, 0.4, 2.0, 1.5, false)];
    let linear = ex(&ExportOpts { segments: vec![blank(0.0, 3.0)], overlays: vec![moving(make_box("kf1.png"), 0.0, 3.0, lin.clone())], ..Default::default() });
    match &linear {
        Ok(m) => {
            let out = Path::new(&m.path);
            let got: Vec<_> = [0.2, 1.5, 2.8].iter().map(|t| bright_box(&frame(out, *t), W, H)).collect();
            let want = [expect(base, (0.0, 0.0, 1.0, 1.0)), expect(base, (0.25, 0.2, 1.5, 1.25)), expect(base, (0.5, 0.4, 2.0, 1.5))];
            let offs: Vec<f64> = got.iter().zip(want).map(|(g, w)| off(*g, w)).collect();
            r.check("moving overlay is at its place and size at three times", offs.iter().all(|o| *o <= 3.0), &format!("off by {offs:.1?} px: {got:.0?} for {want:.0?}"));
        }
        Err(e) => r.check("moving overlay is at its place and size at three times", false, e),
    }

    // --- 2. "Smooth": the same keyframes eased: the same at both keyframes, behind in the first half.
    let eased: Vec<MovePoint> = lin.iter().map(|p| MovePoint { e: true, ..p.clone() }).collect();
    match (&linear, ex(&ExportOpts { segments: vec![blank(0.0, 3.0)], overlays: vec![moving(make_box("kf2.png"), 0.0, 3.0, eased)], ..Default::default() })) {
        (Ok(a), Ok(b)) => {
            let (pa, pb) = (Path::new(&a.path), Path::new(&b.path));
            let at = |p: &Path, t: f64| bright_box(&frame(p, t), W, H);
            let same_keys = off(at(pb, 0.5), at(pa, 0.5).unwrap_or_default()).max(off(at(pb, 2.5), at(pa, 2.5).unwrap_or_default()));
            // At 1.0 s a quarter of the way: even 0.25, eased 0.15625 of the move.
            let (qa, qb) = (at(pa, 1.0), at(pb, 1.0));
            let (wa, wb) = (expect(base, (0.125, 0.1, 1.25, 1.125)), expect(base, (0.078125, 0.0625, 1.15625, 1.078125)));
            r.check("smooth equals even at the keyframes and differs between", same_keys <= 2.0 && off(qa, wa) <= 3.0 && off(qb, wb) <= 3.0 && off(qa, wb) > 10.0, &format!("keys off by {same_keys:.1}; at 1 s even {qa:.0?} (want {wa:.0?}), smooth {qb:.0?} (want {wb:.0?})"));
        }
        (a, b) => r.check("smooth equals even at the keyframes and differs between", false, &format!("{:?} / {:?}", a.as_ref().err(), b.err())),
    }

    // --- 3. A cut between two keyframes (timeline 1 s: no move, 6 s: 0.5 to the right; 1.5..5.5 cut out):
    //        the UI sends both sides of the cut at output 1.5 s, so the box jumps there.
    let cut = vec![mv(0.0, 0.0, 0.0, 1.0, 1.0, false), mv(1.0, 0.0, 0.0, 1.0, 1.0, false), mv(1.5, 0.05, 0.0, 1.0, 1.0, false), mv(1.5, 0.45, 0.0, 1.0, 1.0, false), mv(2.0, 0.5, 0.0, 1.0, 1.0, false), mv(4.0, 0.5, 0.0, 1.0, 1.0, false)];
    match ex(&ExportOpts { segments: vec![blank(0.0, 1.5), blank(5.5, 8.0)], overlays: vec![moving(make_box("kf3.png"), 0.0, 4.0, cut)], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (before, after) = (bright_box(&frame(out, 1.4), W, H), bright_box(&frame(out, 1.6), W, H));
            let (wb, wa) = (expect(base, (0.04, 0.0, 1.0, 1.0)), expect(base, (0.46, 0.0, 1.0, 1.0)));
            r.check("moving overlay jumps at a cut between two keyframes", off(before, wb) <= 3.0 && off(after, wa) <= 3.0, &format!("at 1.4 s {before:.0?} (want {wb:.0?}), at 1.6 s {after:.0?} (want {wa:.0?})"));
        }
        Err(e) => r.check("moving overlay jumps at a cut between two keyframes", false, &e),
    }

    // --- 4. 2x speed: timeline keyframes at 0.5 and 2.5 s are output 0.25 and 1.25 s.
    let fast = vec![mv(0.25, 0.0, 0.0, 1.0, 1.0, false), mv(1.25, 0.5, 0.4, 2.0, 1.5, false)];
    match ex(&ExportOpts { segments: vec![blank(0.0, 3.0)], speed: 2.0, overlays: vec![moving(make_box("kf4.png"), 0.0, 1.5, fast)], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            let (mid, end) = (bright_box(&frame(out, 0.75), W, H), bright_box(&frame(out, 1.4), W, H));
            let (wm, we) = (expect(base, (0.25, 0.2, 1.5, 1.25)), expect(base, (0.5, 0.4, 2.0, 1.5)));
            let len = library::probe(ffmpeg, out, false).map(|p| p.duration).unwrap_or(0.0);
            r.check("moving overlay at 2x speed follows output time", off(mid, wm) <= 8.0 && off(end, we) <= 3.0 && near(len, 1.5, 0.15), &format!("at 0.75 s {mid:.0?} (want {wm:.0?}), at 1.4 s {end:.0?} (want {we:.0?}), length {len:.2}"));
        }
        Err(e) => r.check("moving overlay at 2x speed follows output time", false, &e),
    }

    // --- 5. A moving canvas piece on a 360x640 canvas: from the whole picture across the top to a
    //        zoomed top-left part lower down, over output 0..2 s.
    let (gw, gh) = (40usize, 24usize);
    let inset = |(x, y, w, h): (f64, f64, f64, f64)| (x + w * 0.08, y + h * 0.08, w * 0.84, h * 0.84);
    let same_as = |out: &Path, t: f64, ob: (f64, f64, f64, f64), sb: (f64, f64, f64, f64)| {
        box_diff(&grey_box_at(ffmpeg, out, t, "", inset(ob), gw, gh), &grey_box_at(ffmpeg, src_file, t, "", inset(sb), gw, gh), gw, (0, 0, gw, gh))
    };
    let pp = |t: f64, x: f64, y: f64, w: f64, h: f64, cx: f64, cy: f64, z: f64| PiecePoint { t, x, y, w, h, cx, cy, z, e: false };
    let canvas = Canvas { width: 360.0, height: 640.0, background: "black".into(), pieces: vec![CanvasPiece { sx: 0.0, sy: 0.0, sw: 1.0, sh: 1.0, x: 0.0, y: 0.0, w: 1.0, h: 0.28125 }] };
    let track = vec![pp(0.0, 0.0, 0.0, 1.0, 0.28125, 0.5, 0.5, 0.5625), pp(2.0, 0.1, 0.5, 0.8, 0.225, 0.25, 0.25, 0.9)];
    match ex(&ExportOpts { segments: vec![Range { start: 0.0, end: 2.6, ..Default::default() }], canvas: Some(canvas), canvas_tracks: vec![track], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            // (time, place on the canvas, source box): the source box is the place's size over the zoom, around (cx, cy).
            let cases = [
                (0.0, (0.0, 0.0, 1.0, 0.28125), (0.0, 20.0 / 360.0, 1.0, 320.0 / 360.0)),
                (1.0, (0.05, 0.25, 0.9, 0.253125), ((240.0 - 221.538) / 640.0, (135.0 - 110.769) / 360.0, 443.077 / 640.0, 221.538 / 360.0)),
                (2.4, (0.1, 0.5, 0.8, 0.225), (0.0, 10.0 / 360.0, 0.5, 160.0 / 360.0)),
            ];
            let mut parts = Vec::new();
            let mut ok = (m.width, m.height) == (360, 640);
            for (t, place, sb) in cases {
                let right = same_as(out, t, place, sb);
                // Stretched: a box a fifth narrower in the source (the test picture has upright bars).
                let stretched = same_as(out, t, place, (sb.0, sb.1, sb.2 * 0.8, sb.3));
                ok &= right < 12.0 && stretched > right + 4.0;
                parts.push(format!("{t} s: {right:.1} (stretched {stretched:.1})"));
            }
            let black = grey_box_at(ffmpeg, out, 1.0, "", (0.0, 0.6, 1.0, 0.35), 16, 8).map(|f| box_mean(&f, 16, (0, 0, 16, 8))).unwrap_or(-1.0);
            ok &= (0.0..8.0).contains(&black);
            r.check("moving canvas piece shows the right part at the right place, unstretched, at three times", ok, &format!("{} x {}; {}; black below {black:.1}", m.width, m.height, parts.join(", ")));
        }
        Err(e) => r.check("moving canvas piece shows the right part at the right place, unstretched, at three times", false, &e),
    }

    // --- 6. A moving blur box: blurred where it is at each time, not where it was.
    let blur = BlurBox { x: 0.05, y: 0.05, w: 0.25, h: 0.25, from: 0.0, to: 2.8, strength: 0.03, track: vec![mv(0.0, 0.0, 0.0, 1.0, 1.0, false), mv(2.0, 0.5, 0.45, 1.2, 1.2, false)] };
    match ex(&ExportOpts { segments: vec![Range { start: 0.0, end: 2.8, ..Default::default() }], blurs: vec![blur], ..Default::default() }) {
        Ok(m) => {
            let out = Path::new(&m.path);
            const BW: usize = 320;
            let px = |(x, y, w, h): (f64, f64, f64, f64)| ((x * 320.0) as usize + 4, (y * 180.0) as usize + 4, ((x + w) * 320.0) as usize - 4, ((y + h) * 180.0) as usize - 4);
            let ratio = |t: f64, b: (f64, f64, f64, f64)| {
                let (e, s0) = (grey_at(ffmpeg, out, t, BW, 180), grey_at(ffmpeg, src_file, t, BW, 180));
                let (d, d0) = (box_detail(&e, BW, px(b)), box_detail(&s0, BW, px(b)));
                if d0 > 0.3 { d / d0 } else { 9.0 }
            };
            let start = ratio(0.2, (0.06, 0.06, 0.23, 0.23));
            let end_box = (0.525, 0.475, 0.3, 0.3);
            let (end, left) = (ratio(2.5, end_box), ratio(2.5, (0.05, 0.05, 0.25, 0.25)));
            r.check("moving blur box blurs where it is, not where it was", start < 0.4 && end < 0.4 && (0.75..1.33).contains(&left), &format!("detail ratio at 0.2 s {start:.2}, at 2.5 s in its new place {end:.2}, in its old place {left:.2}"));
        }
        Err(e) => r.check("moving blur box blurs where it is, not where it was", false, &e),
    }

    // --- 7. Many points: the graph is too long for a command line and goes through a file.
    let many: Vec<MovePoint> = (0..600).map(|i| mv(i as f64 * 0.005, i as f64 * 0.001, 0.0, 1.0, 1.0, i % 2 == 0)).collect();
    let before: Vec<_> = fs::read_dir(&odir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect()).unwrap_or_default();
    match ex(&ExportOpts { segments: vec![blank(0.0, 3.0)], overlays: vec![moving(make_box("kf7.png"), 0.0, 3.0, many)], ..Default::default() }) {
        Ok(m) => {
            let late = bright_box(&frame(Path::new(&m.path), 2.8), W, H);
            let left: Vec<_> = fs::read_dir(&odir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "txt") && !before.contains(p)).collect()).unwrap_or_default();
            let want = expect(base, (0.56, 0.0, 1.0, 1.0));
            r.check("a very long keyframe track exports (graph through a file, file removed)", off(late, want) <= 3.0 && left.is_empty(), &format!("{late:.0?} for {want:.0?}; files left {left:?}"));
        }
        Err(e) => r.check("a very long keyframe track exports (graph through a file, file removed)", false, &e),
    }
}

/// Like `band_db`, over `from`..`to` seconds of the file only, and with two
/// band-pass stages: tones a few hundred Hz away are 60+ dB down, not 15.
fn band_db_at(ffmpeg: &Path, file: &Path, stream: u32, freq: u32, from: f64, to: f64) -> Option<f64> {
    let band = format!("bandpass=f={freq}:width_type=q:w=8");
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostats", "-nostdin", "-ss", &format!("{from:.3}"), "-t", &format!("{:.3}", to - from), "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{stream}"), "-af", &format!("{band},{band},volumedetect"), "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse::<f64>().ok()))
}

/// The first frame at or after `t` as 64x36 grey pixels.
fn grey_frame(ffmpeg: &Path, file: &Path, t: f64) -> Option<Vec<u8>> {
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    // Seeking a single JPEG, even to 0, gives no frame at all.
    if t > 0.0 {
        cmd.args(["-ss", &format!("{t:.3}")]);
    }
    let out = cmd
        .arg("-i")
        .arg(file)
        .args(["-frames:v", "1", "-vf", "scale=64:36,format=gray", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.stdout.len() == 64 * 36).then_some(out.stdout)
}

/// Mean absolute difference of two grey frames (255 when either is missing).
fn frame_diff(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64,
        _ => 255.0,
    }
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

/// Checks of the timeline editor: kept ranges, the audio mix, waveforms, film strip and dropped files.
/// The test clip has a still part (no frames) from 3 to 5 s, and its desktop
/// and microphone tracks are tones at `f_desk` and `f_mic`.
fn timeline_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path, (f_desk, f_mic): (u32, u32), base_luma: Option<(f64, f64)>) {
    use std::time::Instant;
    let src_file = Path::new(&src.path);
    let ex = |opts: ExportOpts| -> Result<(ClipMeta, library::Probe, u128), String> {
        let started = Instant::now();
        let m = export(s, src, &opts, &mut |_| {})?;
        let ms = started.elapsed().as_millis();
        let p = library::probe(ffmpeg, Path::new(&m.path), true).ok_or("probe failed")?;
        Ok((m, p, ms))
    };
    let seg = |v: &[(f64, f64)]| v.iter().map(|&(start, end)| Range { start, end, ..Default::default() }).collect::<Vec<_>>();
    let titles_of = |m: &ClipMeta| audio_titles(ffmpeg, Path::new(&m.path)).unwrap_or_default().into_iter().map(Option::unwrap_or_default).collect::<Vec<_>>();
    let desc = |p: &library::Probe, ms: u128| format!("dur={:.3} frames={} audio={} in {ms} ms", p.duration, p.frames, p.audio_streams);
    let three = ["Mix", "Desktop", "Microphone"];
    let track = |index: u32, volume: f32, regions: Vec<GainRegion>| AudioEdit { source: "track".into(), index, volume, regions, ..Default::default() };

    // --- kept ranges
    match ex(ExportOpts { segments: seg(&[(0.5, 2.5), (5.5, 7.5)]), ..Default::default() }) {
        Ok((m, p, ms)) => {
            r.check("two kept ranges last their sum (4 s)", m.kind == "edit" && near(p.duration, 4.0, 0.07), &desc(&p, ms));
            let t = titles_of(&m);
            r.check("two kept ranges keep 3 titled audio tracks", p.audio_streams == 3 && t == three, &format!("{t:?}"));
            // 0.2 s after the join the picture is the source's at 5.7 s.
            let out = grey_frame(ffmpeg, Path::new(&m.path), 2.2);
            let (right, wrong) = (frame_diff(&out, &grey_frame(ffmpeg, src_file, 5.7)), frame_diff(&out, &grey_frame(ffmpeg, src_file, 2.2)));
            r.check("second range carries on from the source's 5.5 s", right < 6.0 && right < wrong, &format!("diff {right:.1} vs {wrong:.1}"));
            // Audio runs through the join: the desktop tone has no gap in it.
            let gap = band_db_at(ffmpeg, Path::new(&m.path), 1, f_desk, 1.9, 2.1).zip(band_db_at(ffmpeg, Path::new(&m.path), 1, f_desk, 1.0, 1.5));
            r.check("no audio gap at the join", gap.is_some_and(|(j, n)| near(j, n, 1.0)), &format!("join, elsewhere {gap:?}"));
        }
        Err(e) => r.check("two kept ranges last their sum (4 s)", false, &e),
    }
    match ex(ExportOpts { segments: seg(&[(0.0, 1.0), (2.0, 3.5), (6.0, 8.0)]), ..Default::default() }) {
        Ok((m, p, ms)) => {
            let t = titles_of(&m);
            r.check("three kept ranges last their sum (4.5 s)", near(p.duration, 4.5, 0.07), &desc(&p, ms));
            r.check("three kept ranges keep 3 titled audio tracks", p.audio_streams == 3 && t == three, &format!("{t:?}"));
        }
        Err(e) => r.check("three kept ranges last their sum (4.5 s)", false, &e),
    }
    // Seven ranges, out of order, one inside another: sorted and merged to six
    // (more than four, so each decoder also runs with fewer threads).
    let seven = seg(&[(4.0, 4.8), (0.0, 0.8), (1.2, 2.0), (2.4, 3.2), (5.2, 6.0), (6.4, 7.2), (5.5, 5.9)]);
    let name = "seven ranges with an overlap merge to six (4.8 s)";
    match ex(ExportOpts { segments: seven, ..Default::default() }) {
        Ok((m, p, ms)) => r.check(name, near(p.duration, 4.8, 0.07) && p.audio_streams == 3 && titles_of(&m) == three, &desc(&p, ms)),
        Err(e) => r.check(name, false, &e),
    }
    match ex(ExportOpts { segments: seg(&[(0.5, 2.5), (5.5, 7.5)]), speed: 2.0, ..Default::default() }) {
        Ok((_, p, ms)) => r.check("two kept ranges at 2.0x last 2 s", near(p.duration, 2.0, 0.07) && p.audio_streams == 3, &desc(&p, ms)),
        Err(e) => r.check("two kept ranges at 2.0x last 2 s", false, &e),
    }
    let bright = ExportOpts { segments: seg(&[(0.5, 2.5), (5.5, 7.5)]), color: Color { brightness: 1.4, ..Default::default() }, ..Default::default() };
    match ex(bright) {
        Ok((m, p, ms)) => {
            let st = signal_stats(ffmpeg, Path::new(&m.path));
            let ok = near(p.duration, 4.0, 0.07) && matches!((base_luma, st), (Some(b), Some(a)) if a.0 > b.0 + 8.0);
            r.check("two kept ranges with brightness 1.4", ok, &format!("{} luma {st:?}", desc(&p, ms)));
        }
        Err(e) => r.check("two kept ranges with brightness 1.4", false, &e),
    }

    // A range that starts in the still part (3..5 s) opens on the picture on
    // screen then (the source's last frame before 3 s), at the join and at the start.
    let still = grey_frame(ffmpeg, src_file, 2.95);
    let after = grey_frame(ffmpeg, src_file, 4.99);
    match ex(ExportOpts { segments: seg(&[(1.0, 2.0), (4.0, 6.0)]), ..Default::default() }) {
        Ok((m, p, ms)) => {
            let out_path = Path::new(&m.path);
            let first = frames_on_screen(ffmpeg, out_path, &[1.001])[0];
            let pic = grey_frame(ffmpeg, out_path, 0.99);
            let lum = pic.as_ref().map(|v| mean(&v.iter().map(|&x| x as f32).collect::<Vec<_>>())).unwrap_or(0.0);
            let (d_still, d_after) = (frame_diff(&pic, &still), frame_diff(&pic, &after));
            let ok = near(p.duration, 3.0, 0.07) && first.is_some_and(|f| f >= 0.98) && lum > 30.0 && d_still < 6.0 && d_still < d_after;
            r.check(
                "join into a still part opens on the picture on screen",
                ok,
                &format!("{} frame at {first:?}, luma {lum:.0}, diff to on-screen {d_still:.1} / to next {d_after:.1}", desc(&p, ms)),
            );
        }
        Err(e) => r.check("join into a still part opens on the picture on screen", false, &e),
    }
    match ex(ExportOpts { segments: seg(&[(4.0, 6.0), (6.5, 7.5)]), ..Default::default() }) {
        Ok((m, p, ms)) => {
            let out_path = Path::new(&m.path);
            let first = frames_on_screen(ffmpeg, out_path, &[0.0])[0];
            let pic = grey_frame(ffmpeg, out_path, 0.0);
            let d_still = frame_diff(&pic, &still);
            let ok = near(p.duration, 3.0, 0.07) && first.is_some_and(|f| f.abs() < 0.01) && d_still < 6.0;
            r.check("export starting in a still part opens on the picture on screen", ok, &format!("{} first frame {first:?}, diff {d_still:.1}", desc(&p, ms)));
        }
        Err(e) => r.check("export starting in a still part opens on the picture on screen", false, &e),
    }

    // --- the audio mix
    let ref_desk = band_db_at(ffmpeg, src_file, 1, f_desk, 0.2, 7.8);
    let ref_mic = band_db_at(ffmpeg, src_file, 2, f_mic, 0.2, 7.8);
    r.line(&format!("INFO source levels desktop {ref_desk:?} microphone {ref_mic:?}"));
    match ex(ExportOpts { audio: vec![track(1, 0.5, vec![])], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let lv = band_db_at(ffmpeg, Path::new(&m.path), 0, f_desk, 0.2, 7.8);
            let ok = p.audio_streams == 1 && titles_of(&m) == ["Desktop"] && matches!((ref_desk, lv), (Some(a), Some(b)) if near(b - a, -6.02, 1.0));
            r.check("track row at volume 0.5 is 6 dB down", ok, &format!("{} level {lv:?} from {ref_desk:?}", desc(&p, ms)));
        }
        Err(e) => r.check("track row at volume 0.5 is 6 dB down", false, &e),
    }
    let regions = vec![GainRegion { start: 1.5, end: 3.5, gain: 0.0 }, GainRegion { start: 4.5, end: 6.5, gain: 2.0 }];
    match ex(ExportOpts { audio: vec![track(1, 1.0, regions)], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let f = Path::new(&m.path);
            let lv = |a: f64, b: f64| band_db_at(ffmpeg, f, 0, f_desk, a, b);
            let (before, muted, loud, after) = (lv(0.2, 1.3), lv(1.8, 3.2), lv(4.8, 6.2), lv(6.8, 7.8));
            let detail = format!("{} before {before:?} gain0 {muted:?} gain2 {loud:?} after {after:?} ref {ref_desk:?}", desc(&p, ms));
            let normal = matches!((ref_desk, before, after), (Some(r0), Some(b), Some(a)) if near(b, r0, 1.0) && near(a, r0, 1.0));
            r.check("gain 0 region is silent, level normal outside it", normal && matches!((ref_desk, muted), (Some(r0), Some(x)) if x < r0 - 40.0), &detail);
            r.check("gain 2 region is 6 dB louder", matches!((ref_desk, loud), (Some(r0), Some(x)) if near(x - r0, 6.02, 1.0)), "");
            // Waveform of a clip that has never had its tracks extracted: read from the clip itself.
            match waveforms(s, &m) {
                Ok(w) if w.len() == 1 && w[0].per_second == 50.0 && w[0].peaks.len() >= 390 => {
                    let pk = &w[0].peaks;
                    let (muted, loud, normal) = (pk[105..145].iter().fold(0.0f32, |a, &b| a.max(b)), mean(&pk[255..295]), mean(&pk[345..385]));
                    r.check("waveform shows the silent and the loud region", muted < 0.01 && loud > normal * 1.6 && normal > 0.05, &format!("silent max {muted:.4}, loud {loud:.3}, normal {normal:.3}"));
                }
                Ok(w) => r.check("waveform shows the silent and the loud region", false, &format!("{} streams, {:?} peaks", w.len(), w.first().map(|x| x.peaks.len()))),
                Err(e) => r.check("waveform shows the silent and the loud region", false, &e),
            }
        }
        Err(e) => r.check("gain 0 region is silent, level normal outside it", false, &e),
    }

    // An audio file at another rate and channel count: 1 s of 1500 Hz, then 5 s of 600 Hz, 44.1 kHz mono.
    let (f_head, f_file) = (1500u32, 600u32);
    let tone = dir.join("Test Tone.wav");
    let built = run_ffmpeg(
        ffmpeg,
        &[
            "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", &format!("sine=frequency={f_head}:sample_rate=44100:duration=1"),
            "-f", "lavfi", "-i", &format!("sine=frequency={f_file}:sample_rate=44100:duration=5"),
            "-filter_complex", "[0:a][1:a]concat=n=2:v=0:a=1[a]", "-map", "[a]", "-ac", "1", "-c:a", "pcm_s16le", &tone.to_string_lossy(),
        ],
    );
    r.check("build test audio file", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    let tone_s = tone.to_string_lossy().into_owned();
    let ref_file = band_db_at(ffmpeg, &tone, 0, f_file, 1.3, 5.7);
    let ref_head = band_db_at(ffmpeg, &tone, 0, f_head, 0.1, 0.9);
    r.line(&format!("INFO file levels {f_file} Hz {ref_file:?}, its first second at {f_head} Hz {ref_head:?}"));
    // From 1 s into the file (past the 1500 Hz part), 3 s of it, heard from 3 s.
    let file_row = AudioEdit { source: "file".into(), path: tone_s.clone(), offset: 3.0, trim_start: 1.0, length: 3.0, volume: 1.0, ..Default::default() };
    match ex(ExportOpts { audio: vec![file_row.clone()], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let f = Path::new(&m.path);
            let lv = |freq: u32, a: f64, b: f64| band_db_at(ffmpeg, f, 0, freq, a, b);
            let (before, during, past, head) = (lv(f_file, 0.3, 2.7), lv(f_file, 3.3, 5.7), lv(f_file, 6.3, 7.7), lv(f_head, 2.5, 5.7));
            let ok = matches!((ref_file, before, during, past), (Some(r0), Some(b), Some(d), Some(a)) if b < r0 - 40.0 && near(d, r0, 1.5) && a < r0 - 40.0)
                && matches!((ref_head, head), (Some(r0), Some(h)) if h < r0 - 40.0)
                && near(p.duration, 8.0, 0.1)
                && titles_of(&m) == ["Test Tone"];
            r.check(
                "file row is silent before its offset, then the trimmed file plays",
                ok,
                &format!("{} before {before:?} during {during:?} after {past:?}, trimmed-off part {head:?}; file {ref_file:?} / {ref_head:?}", desc(&p, ms)),
            );
        }
        Err(e) => r.check("file row is silent before its offset, then the trimmed file plays", false, &e),
    }
    // 0.1.34: a file that plays THROUGH cuts. Cut to 0..2 and 3..6 (5 s), the file from 1 s of the result
    // for 3 s: heard without a break from 1 to 4 s, across the cut at 2 s.
    let through_row = AudioEdit { through: true, offset: 1.0, ..file_row.clone() };
    match ex(ExportOpts { segments: seg(&[(0.0, 2.0), (3.0, 6.0)]), audio: vec![track(2, 1.0, vec![]), through_row.clone()], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let f = Path::new(&m.path);
            let lv = |freq: u32, a: f64, b: f64| band_db_at(ffmpeg, f, 0, freq, a, b);
            let (before, across, after, mic) = (lv(f_file, 0.2, 0.8), lv(f_file, 1.3, 3.7), lv(f_file, 4.3, 4.9), lv(f_mic, 0.3, 4.7));
            let ok = p.audio_streams == 1
                && near(p.duration, 5.0, 0.07)
                && matches!((ref_file, before, across, after), (Some(r0), Some(b), Some(d), Some(a)) if b < r0 - 40.0 && near(d, r0, 1.5) && a < r0 - 40.0)
                && matches!((ref_mic, mic), (Some(r0), Some(x)) if near(x, r0, 1.0));
            r.check("a file that plays through cuts is not cut with the video", ok, &format!("{} file before {before:?} across the cut {across:?} after {after:?}, mic {mic:?}", desc(&p, ms)));
        }
        Err(e) => r.check("a file that plays through cuts is not cut with the video", false, &e),
    }
    // The same file alone (every clip track deleted), at twice the speed: it plays through, sped up with the rest.
    match ex(ExportOpts { segments: seg(&[(0.0, 2.0), (3.0, 6.0)]), speed: 2.0, audio: vec![through_row.clone()], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let f = Path::new(&m.path);
            let heard = band_db_at(ffmpeg, f, 0, f_file, 0.7, 1.8); // the tempo changes, the pitch does not
            let quiet = band_db_at(ffmpeg, f, 0, f_file, 2.15, 2.45);
            let ok = p.audio_streams == 1 && near(p.duration, 2.5, 0.07) && matches!((ref_file, heard, quiet), (Some(r0), Some(h), Some(q)) if near(h, r0, 2.5) && q < r0 - 35.0);
            r.check("a through file alone, at 2x: one track, sped up, as long as the video", ok, &format!("{} heard {heard:?} after it {quiet:?}", desc(&p, ms)));
        }
        Err(e) => r.check("a through file alone, at 2x: one track, sped up, as long as the video", false, &e),
    }
    // Microphone + file, cut to 0..2 and 3..6: the file is heard from 2 s of the result.
    match ex(ExportOpts { segments: seg(&[(0.0, 2.0), (3.0, 6.0)]), audio: vec![track(2, 1.0, vec![]), file_row.clone()], ..Default::default() }) {
        Ok((m, p, ms)) => {
            let f = Path::new(&m.path);
            let lv = |freq: u32, a: f64, b: f64| band_db_at(ffmpeg, f, 0, freq, a, b);
            let (file_a, file_b, mic_a, mic_b) = (lv(f_file, 0.3, 1.7), lv(f_file, 2.3, 4.7), lv(f_mic, 0.3, 1.7), lv(f_mic, 2.3, 4.7));
            let ok = p.audio_streams == 1
                && near(p.duration, 5.0, 0.07)
                && titles_of(&m) == ["Mix"]
                && matches!((ref_file, file_a, file_b), (Some(r0), Some(a), Some(b)) if a < r0 - 40.0 && near(b, r0, 1.5))
                && matches!((ref_mic, mic_a, mic_b), (Some(r0), Some(a), Some(b)) if near(a, r0, 1.0) && near(b, r0, 1.0));
            r.check("microphone + file mixed and cut", ok, &format!("{} file {file_a:?} then {file_b:?}, mic {mic_a:?} then {mic_b:?}", desc(&p, ms)));
        }
        Err(e) => r.check("microphone + file mixed and cut", false, &e),
    }
    let count_before = fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
    let started = Instant::now();
    let missing = AudioEdit { path: dir.join("nope-missing.mp3").to_string_lossy().into_owned(), ..file_row.clone() };
    let bad = export(s, src, &ExportOpts { audio: vec![track(1, 1.0, vec![]), missing], ..Default::default() }, &mut |_| {});
    let ms = started.elapsed().as_millis();
    let count_after = fs::read_dir(dir).map(|d| d.count()).unwrap_or(0);
    let ok = bad.as_ref().err().is_some_and(|e| e.contains("nope-missing.mp3")) && count_before == count_after && ms < 3000;
    r.check("missing audio file is refused cleanly before encoding", ok, &format!("{:?} in {ms} ms", bad.err()));

    // --- waveforms
    let started = Instant::now();
    match waveforms(s, src) {
        Ok(w) => {
            let ms = started.elapsed().as_millis();
            let idx: Vec<u32> = w.iter().map(|x| x.index).collect();
            let lens: Vec<usize> = w.iter().map(|x| x.peaks.len()).collect();
            r.check("waveforms: one per stream", idx == [0, 1, 2] && w.iter().all(|x| x.per_second == 50.0), &format!("{idx:?} in {ms} ms"));
            r.check("waveforms: about 50 peaks a second", lens.iter().all(|&n| (395..=410).contains(&n)), &format!("{lens:?}"));
            let levels: Vec<f32> = w.iter().map(|x| mean(&x.peaks[50..350.min(x.peaks.len())])).collect();
            r.check("waveforms: peaks where the tones are", levels.iter().all(|&l| l > 0.05 && l <= 1.0), &format!("{levels:.3?}"));
            let cache = util::tmp_dir().join(format!("edit-{}", util::sanitize_filename(&src.id))).join("waveforms.json");
            let before = fs::metadata(&cache).and_then(|m| m.modified()).ok();
            let started = Instant::now();
            let again = waveforms(s, src);
            let ms = started.elapsed().as_millis();
            let after = fs::metadata(&cache).and_then(|m| m.modified()).ok();
            r.check("waveforms: second call uses the cache", before.is_some() && before == after && again.is_ok_and(|a| a.len() == 3 && a[1].peaks == w[1].peaks), &format!("{ms} ms"));
        }
        Err(e) => r.check("waveforms: one per stream", false, &e),
    }

    // --- film strip
    let started = Instant::now();
    match thumbs(s, src, 8) {
        Ok(list) => {
            let ms = started.elapsed().as_millis();
            let jpeg = |p: &String| fs::read(p).is_ok_and(|b| b.len() > 100 && b.starts_with(&[0xFF, 0xD8]));
            let size = list.first().and_then(|p| library::probe(ffmpeg, Path::new(p), false)).map(|p| (p.width, p.height));
            r.check("thumbs: 8 JPEG files, 72 px high", list.len() == 8 && list.iter().all(jpeg) && size == Some((128, 72)), &format!("{} files, size {size:?}, {ms} ms", list.len()));
            // Slice 3 is centred on 3.5 s, inside the still part: it shows the picture on screen then.
            let pic = list.get(3).and_then(|p| grey_frame(ffmpeg, Path::new(p), 0.0));
            let (d_still, d_after) = (frame_diff(&pic, &still), frame_diff(&pic, &after));
            r.check("thumbs: a still moment shows the picture on screen", d_still < 12.0 && d_still < d_after, &format!("diff {d_still:.1} vs {d_after:.1}"));
            let times = |l: &[String]| l.iter().map(|p| fs::metadata(p).and_then(|m| m.modified()).ok()).collect::<Vec<_>>();
            let before = times(&list);
            let started = Instant::now();
            let again = thumbs(s, src, 8);
            let ms = started.elapsed().as_millis();
            r.check("thumbs: second call reuses the files", again.as_ref().is_ok_and(|a| *a == list && times(a) == before), &format!("{ms} ms"));
        }
        Err(e) => r.check("thumbs: 8 JPEG files, 72 px high", false, &e),
    }
    // 20 slices of 0.4 s: the short-clip path (one decoder pass for the whole strip).
    let started = Instant::now();
    match thumbs(s, src, 20) {
        Ok(list) => {
            let ms = started.elapsed().as_millis();
            let all = list.len() == 20 && list.iter().all(|p| fs::metadata(p).is_ok_and(|m| m.len() > 100));
            // Slice 9 is centred on 3.8 s (still part); slice 15 on 6.2 s.
            let pic = list.get(9).and_then(|p| grey_frame(ffmpeg, Path::new(p), 0.0));
            let late = list.get(15).and_then(|p| grey_frame(ffmpeg, Path::new(p), 0.0));
            let (d_still, d_late) = (frame_diff(&pic, &still), frame_diff(&late, &grey_frame(ffmpeg, src_file, 6.2)));
            r.check("thumbs: 20 slices of a short clip", all && d_still < 12.0 && d_late < 12.0, &format!("{} files in {ms} ms, still diff {d_still:.1}, 6.2 s diff {d_late:.1}", list.len()));
        }
        Err(e) => r.check("thumbs: 20 slices of a short clip", false, &e),
    }
    match thumbs(s, src, 1) {
        Ok(list) => r.check("thumbs: count clamped to at least 4", list.len() == 4 && list.iter().all(|p| Path::new(p).is_file()), &format!("{}", list.len())),
        Err(e) => r.check("thumbs: count clamped to at least 4", false, &e),
    }

    // --- dropped files
    match audio_info(s, &tone_s) {
        Ok(info) => {
            let peak = info.peaks.iter().fold(0.0f32, |a, &b| a.max(b));
            let ok = info.title == "Test Tone" && near(info.duration, 6.0, 0.05) && info.per_second == 50.0 && (295..=305).contains(&info.peaks.len()) && peak > 0.05 && info.path == tone_s;
            r.check("audio_info on a 44.1 kHz mono file", ok, &format!("title {:?} duration {:.3} peaks {} max {peak:.3}", info.title, info.duration, info.peaks.len()));
        }
        Err(e) => r.check("audio_info on a 44.1 kHz mono file", false, &e),
    }
    let silent_video = dir.join("noaudio.mp4");
    let built = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=gray:s=160x90:r=30:d=1", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", &silent_video.to_string_lossy()]);
    let res = if built.is_ok() { audio_info(s, &silent_video.to_string_lossy()).map(|_| ()) } else { Ok(()) };
    r.check("audio_info refuses a file without audio", res.as_ref().err().is_some_and(|e| e == "That file has no audio in it"), &format!("{res:?}"));
    let res = audio_info(s, &dir.join("nope.wav").to_string_lossy()).map(|_| ());
    r.check("audio_info refuses a missing file", res.is_err(), &format!("{res:?}"));
}

/// `GLINT_EDITOR_BENCH=<clip>`: time the timeline helpers on a real (long) clip, uncached.
fn bench(r: &mut Report, ffmpeg: &Path, s: &Settings, clip: &Path) {
    use std::time::Instant;
    let mut src = library::register(ffmpeg, clip, "Bench", "", "clip");
    src.fps = src.fps.max(1);
    r.line(&format!("INFO bench clip {}x{} {} fps, {:.1} s, {} audio", src.width, src.height, src.fps, src.duration, src.audio_tracks));
    let started = Instant::now();
    let w = waveforms(s, &src);
    r.line(&format!("INFO bench waveforms from the clip: {} ms, {:?}", started.elapsed().as_millis(), w.as_ref().map(|w| w.iter().map(|x| (x.peaks.len(), x.per_second)).collect::<Vec<_>>())));
    let started = Instant::now();
    let t = audio_tracks(s, &src);
    r.line(&format!("INFO bench audio_tracks: {} ms ({:?} tracks)", started.elapsed().as_millis(), t.as_ref().map(Vec::len)));
    if let Ok((dir, _)) = edit_dir(&src) {
        let _ = fs::remove_file(dir.join("waveforms.json"));
    }
    let started = Instant::now();
    let w = waveforms(s, &src);
    r.line(&format!("INFO bench waveforms from extracted tracks: {} ms ({})", started.elapsed().as_millis(), w.is_ok()));
    for count in [20, 60] {
        let started = Instant::now();
        let t = thumbs(s, &src, count);
        r.line(&format!("INFO bench thumbs({count}): {} ms, {:?}", started.elapsed().as_millis(), t.as_ref().map(Vec::len)));
    }
    let d = src.duration;
    let at = |start: f64| Range { start, end: start + 2.0, ..Default::default() };
    let opts = ExportOpts { segments: vec![at(d * 0.1), at(d * 0.5), at(d * 0.9)], ..Default::default() };
    let started = Instant::now();
    let e = export(s, &src, &opts, &mut |_| {});
    r.line(&format!("INFO bench export of 3 x 2 s ranges: {} ms, {:?}", started.elapsed().as_millis(), e.map(|m| m.duration)));
    if let Ok((dir, _)) = edit_dir(&src) {
        let _ = fs::remove_dir_all(dir);
    }
}

/// Presentation times of every video frame of `file`, sorted (seconds from its start).
fn frame_times(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    let start = start_time(&ffprobe, file);
    let Ok(out) = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=pts_time", "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let mut t: Vec<f64> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok()).map(|v| v - start).collect();
    t.sort_by(f64::total_cmp);
    t
}

/// Checks of per-range looks: a color per kept range, blank (black) ranges,
/// and joins between ranges that touch. The test clip moves from 0 to 3 s and
/// from 5 to 8 s at 30 fps (a frame every 1/30 s from 0), its desktop track is
/// a tone at `f_desk`.
fn look_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, f_desk: u32) {
    let src_file = Path::new(&src.path);
    let ex = |opts: ExportOpts| -> Result<(ClipMeta, library::Probe), String> {
        let m = export(s, src, &opts, &mut |_| {})?;
        let p = library::probe(ffmpeg, Path::new(&m.path), true).ok_or("probe failed")?;
        Ok((m, p))
    };
    let grey = Color { saturation: 0.0, ..Default::default() };
    let piece = |start: f64, end: f64, color: Option<Color>, blank: bool| Range { start, end, color, blank };
    let st = |m: &ClipMeta, a: f64, b: f64| signal_stats_at(ffmpeg, Path::new(&m.path), a, b);
    let desc = |m: &ClipMeta, p: &library::Probe| format!("kind={} dur={:.3} frames={}", m.kind, p.duration, p.frames);
    // (luma, saturation) of a window: colorful like the source, grey, or black.
    let colorful = |x: Option<(f64, f64)>| x.is_some_and(|(y, c)| c > 10.0 && y > 30.0);
    let greyish = |x: Option<(f64, f64)>| x.is_some_and(|(y, c)| c < 1.5 && y > 30.0);
    let blackish = |x: Option<(f64, f64)>| x.is_some_and(|(y, c)| y < 17.5 && c < 0.5);

    // --- the opts the UI sends
    let old = serde_json::from_str::<ExportOpts>(r#"{"start":0,"end":0,"speed":1,"color":{"brightness":1,"contrast":1,"saturation":1,"gamma":1},"tracks":[],"segments":[{"start":0.5,"end":2},{"start":3,"end":4}],"audio":[]}"#);
    let ok = matches!(&old, Ok(o) if o.segments.len() == 2 && o.segments.iter().all(|g| g.color.is_none() && !g.blank) && o.segments[1] == Range { start: 3.0, end: 4.0, ..Default::default() });
    r.check("opts JSON without per-range fields", ok, &format!("{:?}", old.map(|o| o.segments)));
    let new = serde_json::from_str::<ExportOpts>(
        r#"{"speed":1,"color":{"brightness":1,"contrast":1,"saturation":1,"gamma":1},"segments":[{"start":0.5,"end":2,"color":{"brightness":1.2,"contrast":1,"saturation":0,"gamma":1}},{"start":2,"end":4,"color":{"brightness":1,"contrast":1,"saturation":1,"gamma":1},"blank":true},{"start":4,"end":5,"color":{"saturation":0.5}}]}"#,
    );
    let ok = matches!(&new, Ok(o) if o.segments.len() == 3
        && o.segments[0].color == Some(Color { brightness: 1.2, saturation: 0.0, ..Default::default() }) && !o.segments[0].blank
        && o.segments[1].blank && o.segments[1].color == Some(Color::default())
        && o.segments[2].color == Some(Color { saturation: 0.5, ..Default::default() }) && !o.segments[2].blank);
    r.check("opts JSON with per-range color and blank", ok, &format!("{:?}", new.map(|o| o.segments)));

    // --- merging
    let ks = |segments: Vec<Range>| kept_ranges(&ExportOpts { segments, ..Default::default() }, 8.0).iter().map(|k| (k.a, k.b, k.blank, is_neutral(&k.color))).collect::<Vec<_>>();
    let same_look = ks(vec![piece(0.0, 1.0, None, false), piece(1.0, 2.0, Some(Color::default()), false)]);
    let touching = ks(vec![piece(0.0, 1.0, None, false), piece(1.0, 2.0, Some(grey.clone()), false), piece(2.0005, 3.0, None, true)]);
    let overlap = ks(vec![piece(1.0, 2.0, Some(grey.clone()), false), piece(0.0, 1.5, None, false)]);
    let ok = same_look == [(0.0, 2.0, false, true)] && touching == [(0.0, 1.0, false, true), (1.0, 2.0, false, false), (2.0, 3.0, true, true)] && overlap == [(0.0, 1.5, false, true), (1.5, 2.0, false, false)];
    r.check("kept ranges merge only where they look the same", ok, &format!("{same_look:?} / {touching:?} / {overlap:?}"));

    // --- a color per range: colorful, grey, colorful; the second join touches
    let opts = ExportOpts { segments: vec![piece(0.5, 2.5, None, false), piece(5.5, 6.5, Some(grey.clone()), false), piece(6.5, 7.5, None, false)], ..Default::default() };
    match ex(opts) {
        Ok((m, p)) => {
            // Result: 0..2 s colorful, 2..3 s grey, 3..4 s colorful.
            let (a, b, c) = (st(&m, 0.2, 1.8), st(&m, 2.1, 2.9), st(&m, 3.1, 3.9));
            let src_b = signal_stats_at(ffmpeg, src_file, 5.6, 6.4);
            let luma_kept = matches!((b, src_b), (Some(x), Some(y)) if near(x.0, y.0, 6.0));
            let ok = m.kind == "edit" && near(p.duration, 4.0, 0.07) && colorful(a) && greyish(b) && luma_kept && colorful(c);
            r.check("per-range saturation 0 greys that range only", ok, &format!("{} ranges {a:?} {b:?} {c:?}, source of the grey one {src_b:?}", desc(&m, &p)));
        }
        Err(e) => r.check("per-range saturation 0 greys that range only", false, &e),
    }

    // --- touching ranges: the border frame. At 2.0 s the source has a frame,
    // so 0.5..2.0 + 2.0..2.9 must give the same 72 frames as 0.5..2.9 in one.
    let spacing = |t: &[f64]| t.windows(2).map(|w| w[1] - w[0]).fold((f64::INFINITY, 0.0f64), |(lo, hi), d| (lo.min(d), hi.max(d)));
    let one = ex(ExportOpts { segments: vec![piece(0.5, 2.9, Some(grey.clone()), false)], ..Default::default() }).map(|(m, _)| frame_times(ffmpeg, Path::new(&m.path)));
    let two = ex(ExportOpts { segments: vec![piece(0.5, 2.0, None, false), piece(2.0, 2.9, Some(grey.clone()), false)], ..Default::default() });
    match (one, two) {
        (Ok(one), Ok((m, p))) => {
            let f = Path::new(&m.path);
            let t = frame_times(ffmpeg, f);
            let (lo, hi) = spacing(&t);
            r.line(&format!("INFO touching join on a frame: {} frames (one range: {}), spacing {lo:.4}..{hi:.4} s", t.len(), one.len()));
            let ok = t.len() == 72 && one.len() == 72 && lo > 0.030 && hi < 0.037 && near(p.duration, 2.4, 0.07);
            r.check("touching ranges join without a repeated or lost frame", ok, &format!("{} frames, one range {}, spacing {lo:.4}..{hi:.4}", t.len(), one.len()));
            let gap = band_db_at(ffmpeg, f, 1, f_desk, 1.4, 1.6).zip(band_db_at(ffmpeg, f, 1, f_desk, 0.5, 1.0));
            r.check("no audio gap where ranges touch", gap.is_some_and(|(j, n)| near(j, n, 1.0)), &format!("join, elsewhere {gap:?}"));
            let (a, b) = (st(&m, 0.1, 1.4), st(&m, 1.6, 2.3));
            r.check("touching ranges each keep their own color", colorful(a) && greyish(b), &format!("{a:?} then {b:?}"));
        }
        (Err(e), _) | (_, Err(e)) => r.check("touching ranges join without a repeated or lost frame", false, &e),
    }
    // Between frames (2.01 s): the frame on screen (2.0 s) ends the first range
    // uncorrected and opens the second one grey, so it is there twice, 10 ms apart.
    match ex(ExportOpts { segments: vec![piece(0.5, 2.01, None, false), piece(2.01, 2.9, Some(grey.clone()), false)], ..Default::default() }) {
        Ok((m, p)) => {
            let t = frame_times(ffmpeg, Path::new(&m.path));
            let (lo, hi) = spacing(&t);
            r.line(&format!("INFO touching join between frames: {} frames, spacing {lo:.4}..{hi:.4} s", t.len()));
            let ok = t.len() == 73 && lo > 0.008 && hi < 0.037 && near(p.duration, 2.4, 0.07);
            r.check("touching ranges between frames repeat the frame on screen once", ok, &format!("{} frames, spacing {lo:.4}..{hi:.4}", t.len()));
        }
        Err(e) => r.check("touching ranges between frames repeat the frame on screen once", false, &e),
    }

    // --- blank: black picture, audio and length kept. The last range runs to the clip's end.
    let ref_desk = band_db_at(ffmpeg, src_file, 1, f_desk, 5.6, 7.4);
    let opts = ExportOpts { segments: vec![piece(0.5, 2.5, None, false), piece(5.5, 7.5, None, true), piece(7.5, 8.0, None, false)], ..Default::default() };
    match ex(opts) {
        Ok((m, p)) => {
            let f = Path::new(&m.path);
            // Result: 0..2 s picture, 2..4 s black, 4..4.5 s picture.
            let (a, b, c) = (st(&m, 0.2, 1.8), st(&m, 2.1, 3.9), st(&m, 4.1, 4.45));
            let t = frame_times(ffmpeg, f);
            let in_blank = t.iter().filter(|&&x| (2.0..3.999).contains(&x)).count();
            let lv = band_db_at(ffmpeg, f, 1, f_desk, 2.2, 3.8);
            let ok = near(p.duration, 4.5, 0.07) && p.audio_streams == 3 && colorful(a) && blackish(b) && colorful(c) && (58..=61).contains(&in_blank);
            r.check("blank range is black, as long as the range, with its frames", ok, &format!("{} ranges {a:?} {b:?} {c:?}, {in_blank} frames in the blank 2 s", desc(&m, &p)));
            r.check("blank range keeps its audio", matches!((ref_desk, lv), (Some(x), Some(y)) if near(x, y, 1.0)), &format!("{lv:?} source {ref_desk:?}"));
        }
        Err(e) => r.check("blank range is black, as long as the range, with its frames", false, &e),
    }
    // With speed, on libx264 (the fallback encoder): 2 + 2 s at 2.0x.
    let opts = ExportOpts { speed: 2.0, segments: vec![piece(0.5, 2.5, Some(grey.clone()), false), piece(5.5, 7.5, None, true)], ..Default::default() };
    let cpu = Settings { encoder: "x264".into(), ..s.clone() };
    match export(&cpu, src, &opts, &mut |_| {}).and_then(|m| library::probe(ffmpeg, Path::new(&m.path), true).map(|p| (m, p)).ok_or("probe failed".into())) {
        Ok((m, p)) => {
            let (a, b) = (st(&m, 0.1, 0.9), st(&m, 1.1, 1.9));
            r.check("grey + blank ranges at 2.0x with libx264", near(p.duration, 2.0, 0.07) && greyish(a) && blackish(b), &format!("{} {a:?} {b:?}", desc(&m, &p)));
        }
        Err(e) => r.check("grey + blank ranges at 2.0x with libx264", false, &e),
    }

    // --- the top-level color
    let two = |c: [Option<Color>; 2]| {
        let [x, y] = c;
        vec![piece(0.5, 2.5, x, false), piece(5.5, 7.5, y, false)]
    };
    match ex(ExportOpts { color: grey.clone(), segments: two([None, None]), ..Default::default() }) {
        Ok((m, p)) => {
            let (a, b) = (st(&m, 0.2, 1.8), st(&m, 2.2, 3.8));
            r.check("top-level saturation 0 greys every range", near(p.duration, 4.0, 0.07) && greyish(a) && greyish(b), &format!("{} {a:?} {b:?}", desc(&m, &p)));
        }
        Err(e) => r.check("top-level saturation 0 greys every range", false, &e),
    }
    match ex(ExportOpts { color: grey.clone(), segments: two([None, Some(Color::default())]), ..Default::default() }) {
        Ok((m, _)) => {
            let (a, b) = (st(&m, 0.2, 1.8), st(&m, 2.2, 3.8));
            r.check("a range's own color replaces the top-level one", greyish(a) && colorful(b), &format!("{a:?} {b:?}"));
        }
        Err(e) => r.check("a range's own color replaces the top-level one", false, &e),
    }
    match ex(ExportOpts { segments: two([Some(grey.clone()), Some(grey.clone())]), ..Default::default() }) {
        Ok((m, _)) => {
            let (a, b) = (st(&m, 0.2, 1.8), st(&m, 2.2, 3.8));
            r.check("the same own color on every range greys them all", greyish(a) && greyish(b), &format!("{a:?} {b:?}"));
        }
        Err(e) => r.check("the same own color on every range greys them all", false, &e),
    }

    // --- the lossless cut is only for a plain range
    match ex(ExportOpts { segments: vec![piece(1.0, 2.5, Some(grey.clone()), false)], ..Default::default() }) {
        Ok((m, p)) => {
            let a = st(&m, 0.1, 1.4);
            r.check("one range with its own color is re-encoded", m.kind == "edit" && near(p.duration, 1.5, 0.07) && greyish(a), &format!("{} {a:?}", desc(&m, &p)));
        }
        Err(e) => r.check("one range with its own color is re-encoded", false, &e),
    }
    match ex(ExportOpts { segments: vec![piece(1.0, 2.5, None, true)], ..Default::default() }) {
        Ok((m, p)) => {
            let a = st(&m, 0.1, 1.4);
            r.check("one blank range is re-encoded black", m.kind == "edit" && near(p.duration, 1.5, 0.07) && p.audio_streams == 3 && blackish(a), &format!("{} {a:?}", desc(&m, &p)));
        }
        Err(e) => r.check("one blank range is re-encoded black", false, &e),
    }
    // Its own neutral color replaces a top-level one: nothing left to change.
    match ex(ExportOpts { color: grey.clone(), segments: vec![piece(1.0, 2.5, Some(Color::default()), false)], ..Default::default() }) {
        Ok((m, p)) => r.check("one range with its own neutral color uses the lossless cut", m.kind == "trim", &desc(&m, &p)),
        Err(e) => r.check("one range with its own neutral color uses the lossless cut", false, &e),
    }
}

/// Checks of `ExportOpts::ordered` (clips moved around in an "Edit together"
/// draft) and of exports from a draft. The test clip here moves for 8 s and
/// its one audio track ("Desktop") is a tone that steps every 2 s, so each
/// stretch of source time is told apart by its picture and by its pitch.
fn ordered_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    // Far enough apart that the double band-pass of `band_db_at` holds each 35+ dB above the others.
    let steps = [300u32, 700, 1500, 4000];
    let tone_at = |t: f64| steps[((t / 2.0).floor() as usize).min(3)];
    let src_file = dir.join("steps.mp4");
    let built = run_ffmpeg(
        ffmpeg,
        &[
            "-hide_banner", "-loglevel", "error", "-y",
            "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=8",
            "-f", "lavfi", "-i", "aevalsrc='0.5*sin(2*PI*t*if(lt(t,2),300,if(lt(t,4),700,if(lt(t,6),1500,4000))))':s=48000:d=8",
            "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", "-force_key_frames", "expr:gte(t,n_forced*1)",
            "-c:a", "aac", "-b:a", "160k", "-ac", "2", "-metadata:s:a:0", "title=Desktop", "-movflags", "+faststart", &src_file.to_string_lossy(),
        ],
    );
    r.check("ordered: build stepped test clip", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    if built.is_err() {
        return;
    }
    let mut src = library::register(ffmpeg, &src_file, "Steps", "Test Game", "clip");
    src.fps = 30;
    src.folder = "Test Game".into();
    let ex = |src: &ClipMeta, opts: ExportOpts| -> Result<(ClipMeta, library::Probe), String> {
        let m = export(s, src, &opts, &mut |_| {})?;
        let p = library::probe(ffmpeg, Path::new(&m.path), true).ok_or("probe failed")?;
        Ok((m, p))
    };
    let piece = |start: f64, end: f64, color: Option<Color>, blank: bool| Range { start, end, color, blank };
    let refs: Vec<Option<f64>> = (0..4).map(|i| band_db_at(ffmpeg, &src_file, 0, steps[i], i as f64 * 2.0 + 0.2, i as f64 * 2.0 + 1.8)).collect();
    r.line(&format!("INFO stepped clip tone levels {refs:?}"));
    // Out of source order; the first one runs to the clip's end.
    let order = [(6.5, 8.0), (0.3, 1.8), (4.2, 5.6)];
    let total: f64 = order.iter().map(|(a, b)| b - a).sum();
    // Each range's picture and pitch at its place in the result.
    let per_range = |out: &Path| -> (bool, String) {
        let mut ok = true;
        let mut detail = String::new();
        let mut at = 0.0;
        for &(a, b) in &order {
            let pic = grey_frame(ffmpeg, out, at + 0.5);
            let (right, wrong) = (frame_diff(&pic, &grey_frame(ffmpeg, &src_file, a + 0.5)), frame_diff(&pic, &grey_frame(ffmpeg, &src_file, at + 0.5)));
            let own = tone_at(a);
            let lv = band_db_at(ffmpeg, out, 0, own, at + 0.15, at + (b - a) - 0.15);
            let others: Vec<Option<f64>> = order.iter().map(|o| tone_at(o.0)).filter(|f| *f != own).map(|f| band_db_at(ffmpeg, out, 0, f, at + 0.15, at + (b - a) - 0.15)).collect();
            let r0 = refs[steps.iter().position(|f| *f == own).unwrap_or(0)];
            let sound = matches!((r0, lv), (Some(x), Some(y)) if near(x, y, 1.5)) && others.iter().all(|o| matches!((r0, o), (Some(x), Some(y)) if *y < x - 30.0));
            ok &= right < 6.0 && right < wrong && sound;
            detail += &format!("[{a}..{b} at {at:.1}: picture diff {right:.1} vs {wrong:.1}, {own} Hz {lv:.1?} others {others:.1?}] ");
            at += b - a;
        }
        (ok, detail)
    };

    // --- order, picture and sound
    let segs = order.iter().map(|&(a, b)| piece(a, b, None, false)).collect::<Vec<_>>();
    match ex(&src, ExportOpts { segments: segs.clone(), ordered: true, ..Default::default() }) {
        Ok((m, p)) => {
            let out = Path::new(&m.path);
            r.check("ordered: ranges out of source order last their sum", m.kind == "edit" && near(p.duration, total, 0.07) && p.audio_streams == 1, &format!("dur {:.3} for {total:.3}, audio {}", p.duration, p.audio_streams));
            let (ok, detail) = per_range(out);
            r.check("ordered: each range's picture and sound, in the order given", ok, &detail);
        }
        Err(e) => r.check("ordered: ranges out of source order last their sum", false, &e),
    }
    // The same list without `ordered` is sorted, as before.
    match ex(&src, ExportOpts { segments: segs.clone(), ..Default::default() }) {
        Ok((m, _)) => {
            let pic = grey_frame(ffmpeg, Path::new(&m.path), 0.5);
            let d = frame_diff(&pic, &grey_frame(ffmpeg, &src_file, 0.8));
            r.check("not ordered: the same ranges play in source order", d < 6.0, &format!("diff to the source at 0.8 s {d:.1}"));
        }
        Err(e) => r.check("not ordered: the same ranges play in source order", false, &e),
    }

    // --- with a look per range: grey, blank, as recorded
    let grey = Color { saturation: 0.0, ..Default::default() };
    let looks = vec![piece(6.5, 8.0, Some(grey.clone()), false), piece(0.3, 1.8, None, true), piece(4.2, 5.6, None, false)];
    match ex(&src, ExportOpts { segments: looks, ordered: true, ..Default::default() }) {
        Ok((m, p)) => {
            let f = Path::new(&m.path);
            let (a, b, c) = (signal_stats_at(ffmpeg, f, 0.1, 1.4), signal_stats_at(ffmpeg, f, 1.6, 2.9), signal_stats_at(ffmpeg, f, 3.1, 4.3));
            let grey_ok = a.is_some_and(|(y, c)| c < 1.5 && y > 30.0);
            let black_ok = b.is_some_and(|(y, c)| y < 17.5 && c < 0.5);
            let color_ok = c.is_some_and(|(y, c)| c > 10.0 && y > 30.0);
            let blank_sound = band_db_at(ffmpeg, f, 0, steps[0], 1.65, 2.85);
            let ok = near(p.duration, total, 0.07) && grey_ok && black_ok && color_ok && matches!((refs[0], blank_sound), (Some(x), Some(y)) if near(x, y, 1.5));
            r.check("ordered: grey, blank and plain ranges keep their look and sound", ok, &format!("dur {:.3}, {a:?} {b:?} {c:?}, blank range's tone {blank_sound:.1?}", p.duration));
        }
        Err(e) => r.check("ordered: grey, blank and plain ranges keep their look and sound", false, &e),
    }

    // --- a file row given as two entries (one per moved clip it covers):
    // 1.5 s of 800 Hz, then 1.5 s of 2500 Hz, heard under the first two ranges.
    let song = dir.join("Two Part Song.wav");
    let built = run_ffmpeg(
        ffmpeg,
        &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "aevalsrc='0.5*sin(2*PI*t*if(lt(t,1.5),800,2500))':s=48000:d=3", "-ac", "2", "-c:a", "pcm_s16le", &song.to_string_lossy()],
    );
    r.check("ordered: build two-part audio file", built.is_ok(), built.as_ref().err().map(String::as_str).unwrap_or(""));
    let song_s = song.to_string_lossy().into_owned();
    let (s1, s2) = (800u32, 2500u32);
    let (ref1, ref2) = (band_db_at(ffmpeg, &song, 0, s1, 0.15, 1.35), band_db_at(ffmpeg, &song, 0, s2, 1.65, 2.85));
    let entry = |offset: f64, trim_start: f64, length: f64| AudioEdit { source: "file".into(), path: song_s.clone(), offset, trim_start, length, volume: 1.0, ..Default::default() };
    let entries = vec![entry(6.5, 0.0, 1.5), entry(0.3, 1.5, 1.5)];
    let track0 = AudioEdit { source: "track".into(), index: 0, volume: 1.0, ..Default::default() };
    let mut audio = vec![track0];
    audio.extend(entries.iter().cloned());
    match ex(&src, ExportOpts { segments: segs.clone(), ordered: true, audio, ..Default::default() }) {
        Ok((m, p)) => {
            let f = Path::new(&m.path);
            let lv = |freq: u32, a: f64, b: f64| band_db_at(ffmpeg, f, 0, freq, a, b);
            let (a6, a9, b6, b9, c6, c9) = (lv(s1, 0.15, 1.35), lv(s2, 0.15, 1.35), lv(s1, 1.65, 2.85), lv(s2, 1.65, 2.85), lv(s1, 3.15, 4.25), lv(s2, 3.15, 4.25));
            let (t_a, t_c) = (lv(steps[3], 0.15, 1.35), lv(steps[2], 3.15, 4.25));
            let (ref600, ref900) = (ref1, ref2);
            let loud = |x: Option<f64>, r0: Option<f64>| matches!((r0, x), (Some(r0), Some(x)) if near(x, r0, 1.5));
            let quiet = |x: Option<f64>, r0: Option<f64>| matches!((r0, x), (Some(r0), Some(x)) if x < r0 - 30.0);
            let titles = audio_titles(ffmpeg, f).unwrap_or_default();
            let ok = near(p.duration, total, 0.07)
                && loud(a6, ref600) && quiet(a9, ref900)
                && loud(b9, ref900) && quiet(b6, ref600)
                && quiet(c6, ref600) && quiet(c9, ref900)
                && loud(t_a, refs[3]) && loud(t_c, refs[2])
                && titles == [Some("Mix".to_string())];
            r.check(
                "ordered: a file row given as two entries plays each part under its clip",
                ok,
                &format!("file {s1}/{s2} Hz: first {a6:.1?}/{a9:.1?}, second {b6:.1?}/{b9:.1?}, third {c6:.1?}/{c9:.1?}; clip tones {t_a:.1?} {t_c:.1?}; file {ref600:.1?}/{ref900:.1?}; titles {titles:?}"),
            );
        }
        Err(e) => r.check("ordered: a file row given as two entries plays each part under its clip", false, &e),
    }
    match ex(&src, ExportOpts { segments: segs.clone(), ordered: true, audio: entries, ..Default::default() }) {
        Ok((m, _)) => {
            let titles = audio_titles(ffmpeg, Path::new(&m.path)).unwrap_or_default();
            r.check("ordered: a file row in two entries alone keeps the file's name", titles == [Some("Two Part Song".to_string())], &format!("{titles:?}"));
        }
        Err(e) => r.check("ordered: a file row in two entries alone keeps the file's name", false, &e),
    }

    // --- merging neighbours
    let ks = |segments: Vec<Range>, ordered: bool| kept_ranges(&ExportOpts { segments, ordered, ..Default::default() }, 8.0).iter().map(|k| (k.a, k.b, k.blank)).collect::<Vec<_>>();
    let p = |a: f64, b: f64| piece(a, b, None, false);
    let merged = ks(vec![p(4.0, 5.0), p(5.0, 6.0), p(0.5, 1.5)], true);
    let apart = ks(vec![p(5.0, 6.0), p(0.5, 1.5), p(6.0, 7.0)], true);
    let backwards = ks(vec![p(5.0, 6.0), p(4.0, 5.0)], true);
    let looks_differ = ks(vec![p(4.0, 5.0), piece(5.0005, 6.0, None, true)], true);
    let sorted = ks(vec![p(5.0, 6.0), p(4.0, 5.0)], false);
    let ok = merged == [(4.0, 6.0, false), (0.5, 1.5, false)]
        && apart == [(5.0, 6.0, false), (0.5, 1.5, false), (6.0, 7.0, false)]
        && backwards == [(5.0, 6.0, false), (4.0, 5.0, false)]
        && looks_differ == [(4.0, 5.0, false), (5.0, 6.0, true)]
        && sorted == [(4.0, 6.0, false)];
    r.check("ordered: only list neighbours that touch in source and look alike merge", ok, &format!("{merged:?} / {apart:?} / {backwards:?} / {looks_differ:?} / not ordered {sorted:?}"));
    let neighbours = vec![p(4.0, 5.0), p(5.0, 6.0), p(0.5, 1.5)];
    match ex(&src, ExportOpts { segments: neighbours, ordered: true, ..Default::default() }) {
        Ok((m, pr)) => {
            let f = Path::new(&m.path);
            let t = frame_times(ffmpeg, f);
            // 2 s + 1 s at 30 fps; no tone break inside the merged 4..6 s.
            let steady = band_db_at(ffmpeg, f, 0, steps[2], 0.2, 1.8);
            let ok = near(pr.duration, 3.0, 0.07) && (88..=91).contains(&t.len()) && matches!((refs[2], steady), (Some(x), Some(y)) if near(x, y, 1.5));
            r.check("ordered: touching neighbours play as one range", ok, &format!("dur {:.3}, {} frames, 4..6 s tone {steady:.1?}", pr.duration, t.len()));
        }
        Err(e) => r.check("ordered: touching neighbours play as one range", false, &e),
    }
    match ex(&src, ExportOpts { segments: vec![p(5.0, 6.0), p(4.0, 5.0)], ordered: true, ..Default::default() }) {
        Ok((m, pr)) => {
            let f = Path::new(&m.path);
            let pic = grey_frame(ffmpeg, f, 1.5);
            let (right, wrong) = (frame_diff(&pic, &grey_frame(ffmpeg, &src_file, 4.5)), frame_diff(&pic, &grey_frame(ffmpeg, &src_file, 5.5)));
            r.check("ordered: ranges that touch in source the other way round stay apart", near(pr.duration, 2.0, 0.07) && right < 6.0 && right < wrong, &format!("dur {:.3}, diff {right:.1} vs {wrong:.1}", pr.duration));
        }
        Err(e) => r.check("ordered: ranges that touch in source the other way round stay apart", false, &e),
    }

    draft_checks(r, ffmpeg, s, dir, &src, &order);
}

/// Exports from a draft ("Edit together"): into its folder in the clips
/// folder (or the clips folder itself when that is gone), never a draft; a
/// lossless whole export is the joined clip itself.
fn draft_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path, plain: &ClipMeta, order: &[(f64, f64)]) {
    let drafts = dir.join(".drafts");
    let _ = fs::create_dir_all(&drafts);
    let _ = fs::create_dir_all(dir.join("Draft Game"));
    let make = |folder: &str| -> Option<ClipMeta> {
        let id = util::new_id();
        let file = drafts.join(format!("{id}.mp4"));
        fs::copy(&plain.path, &file).ok()?;
        let mut m = library::register(ffmpeg, &file, "Joined clips", "Draft Game", "joined");
        m.fps = 30;
        m.folder = folder.into();
        m.draft = true;
        m.joins = vec![3.0, 5.5];
        m.join_names = vec!["One".into(), "Two".into(), "Three".into()];
        Some(m)
    };
    let Some(draft) = make("Draft Game") else {
        r.check("draft: test draft made", false, "");
        return;
    };
    let in_folder = |m: &ClipMeta, sub: &str| {
        let want = if sub.is_empty() { dir.to_path_buf() } else { dir.join(sub) };
        Path::new(&m.path).parent() == Some(want.as_path()) && m.folder == sub && !m.draft
    };
    let ex = |src: &ClipMeta, opts: ExportOpts| export(s, src, &opts, &mut |_| {});
    let parts = vec![Range { start: 0.0, end: 3.0, ..Default::default() }, Range { start: 3.0, end: 5.5, ..Default::default() }, Range { start: 5.5, end: 8.0, ..Default::default() }];
    for (label, opts) in [("nothing changed", ExportOpts::default()), ("its parts as they are", ExportOpts { segments: parts, ..Default::default() })] {
        match ex(&draft, opts) {
            Ok(m) => {
                let ok = in_folder(&m, "Draft Game") && m.kind == "joined" && m.title == "Joined clips" && m.joins == draft.joins && m.join_names == draft.join_names && near(m.duration, draft.duration, 0.05);
                r.check(&format!("draft: lossless whole export ({label}) is the joined clip"), ok, &format!("{} kind {} title {:?} joins {:?} names {:?} folder {:?} draft {}", m.path, m.kind, m.title, m.joins, m.join_names, m.folder, m.draft));
            }
            Err(e) => r.check(&format!("draft: lossless whole export ({label}) is the joined clip"), false, &e),
        }
    }
    match ex(&draft, ExportOpts { start: 1.0, end: 5.0, ..Default::default() }) {
        Ok(m) => {
            let ok = in_folder(&m, "Draft Game") && m.kind == "trim" && m.title == "Joined clips (trim)" && m.joins.is_empty() && m.join_names.is_empty();
            r.check("draft: a lossless cut is a trim without joins, in the draft's folder", ok, &format!("{} kind {} title {:?} joins {:?}", m.path, m.kind, m.title, m.joins));
        }
        Err(e) => r.check("draft: a lossless cut is a trim without joins, in the draft's folder", false, &e),
    }
    let moved = order.iter().map(|&(start, end)| Range { start, end, ..Default::default() }).collect::<Vec<_>>();
    match ex(&draft, ExportOpts { segments: moved, ordered: true, ..Default::default() }) {
        Ok(m) => {
            let ok = in_folder(&m, "Draft Game") && m.kind == "edit" && m.title == "Joined clips (edit)" && m.joins.is_empty() && m.join_names.is_empty();
            r.check("draft: a re-encoded export is an edit without joins, in the draft's folder", ok, &format!("{} kind {} title {:?} joins {:?}", m.path, m.kind, m.title, m.joins));
        }
        Err(e) => r.check("draft: a re-encoded export is an edit without joins, in the draft's folder", false, &e),
    }
    match make("Gone Game").map(|d| (ex(&d, ExportOpts::default()), ex(&d, ExportOpts { speed: 2.0, ..Default::default() }))) {
        Some((Ok(a), Ok(b))) => {
            let ok = in_folder(&a, "") && in_folder(&b, "") && !dir.join("Gone Game").exists();
            r.check("draft: with its folder gone, exports go to the clips folder", ok, &format!("{} ({:?}) / {} ({:?})", a.path, a.folder, b.path, b.folder));
        }
        Some((Err(e), _)) | Some((_, Err(e))) => r.check("draft: with its folder gone, exports go to the clips folder", false, &e),
        None => r.check("draft: with its folder gone, exports go to the clips folder", false, "draft not made"),
    }
    let left = fs::read_dir(&drafts).map(|d| d.count()).unwrap_or(0);
    r.check("draft: nothing written into the drafts folder", left == 2, &format!("{left} files"));
}

/// Set a folder's modified time `age` into the past (to test the stale-folder sweep).
fn age_dir(dir: &Path, age: Duration) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let when = std::time::SystemTime::now().checked_sub(age).ok_or("bad age")?;
    let f = fs::OpenOptions::new().access_mode(FILE_WRITE_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(dir).map_err(|e| e.to_string())?;
    f.set_modified(when).map_err(|e| e.to_string())
}

/// Grey 128 through brightness/contrast and gamma must land where the CSS formulas say.
fn color_formula_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    let grey_file = dir.join("grey.mp4");
    let built = run_ffmpeg(
        ffmpeg,
        &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=0x808080:s=320x240:r=30:d=2", "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p", &grey_file.to_string_lossy()],
    );
    if let Err(e) = built {
        r.check("build grey clip", false, &e);
        return;
    }
    let mut grey = library::register(ffmpeg, &grey_file, "Grey", "", "clip");
    grey.fps = 30;
    let Some((y0, _)) = signal_stats(ffmpeg, &grey_file) else {
        r.check("measure grey clip", false, "");
        return;
    };
    // Back to 0..1 RGB through limited-range luma.
    let v0 = ((y0 - 16.0) / 219.0).clamp(0.0, 1.0);
    let to_y = |v: f64| 16.0 + 219.0 * v.clamp(0.0, 1.0);
    let cases = [
        ("brightness 1.25 contrast 1.5", Color { brightness: 1.25, contrast: 1.5, ..Default::default() }, to_y(((v0 * 1.25).min(1.0) - 0.5) * 1.5 + 0.5)),
        ("gamma 2.0", Color { gamma: 2.0, ..Default::default() }, to_y(v0.powf(0.5))),
        ("brightness 0.5 gamma 0.5", Color { brightness: 0.5, gamma: 0.5, ..Default::default() }, to_y((v0 * 0.5).powf(2.0))),
    ];
    for (name, c, want) in cases {
        let opts = ExportOpts { color: c, tracks: vec![], ..Default::default() };
        let got = export(s, &grey, &opts, &mut |_| {}).ok().and_then(|m| signal_stats(ffmpeg, Path::new(&m.path)));
        let ok = got.is_some_and(|(y, _)| near(y, want, 3.0));
        r.check(&format!("color formula: {name}"), ok, &format!("luma {:?} expected {want:.1} (source {y0:.1})", got.map(|g| g.0)));
    }
}

// ---------------------------------------------------------------------------
// Timeline helpers: waveforms, film strip, dropped audio files

/// Waveforms are drawn from audio decoded at this rate, mono: plenty for peak
/// levels (it keeps everything under 3.6 kHz) and cheap to read back.
const WAVE_RATE: u32 = 8_000;
/// Peaks per second, and the most one stream gets (long clips get fewer per second).
const PEAKS_PER_SECOND: f64 = 50.0;
const MAX_PEAKS: f64 = 6_000.0;

fn peaks_per_second(duration: f64) -> f32 {
    if duration > 0.0 && duration * PEAKS_PER_SECOND > MAX_PEAKS { (MAX_PEAKS / duration) as f32 } else { PEAKS_PER_SECOND as f32 }
}

/// Peak level (0..1) per slice of raw mono s16le audio at `WAVE_RATE`, and the
/// number of samples read. Streams, so an hour of audio never sits in memory.
fn read_peaks(mut reader: impl Read, per_second: f32) -> (Vec<f32>, u64) {
    let per_peak = WAVE_RATE as f64 / per_second.max(0.01) as f64;
    let mut peaks = Vec::new();
    let (mut cur, mut n, mut edge) = (0.0f32, 0u64, per_peak);
    let mut buf = vec![0u8; 1 << 16];
    let mut odd: Option<u8> = None;
    loop {
        let got = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut bytes = &buf[..got];
        let mut sample = |v: i16| {
            if n as f64 >= edge {
                peaks.push(cur.min(1.0));
                cur = 0.0;
                edge += per_peak;
            }
            cur = cur.max((v as f32 / 32768.0).abs());
            n += 1;
        };
        if let Some(lo) = odd.take() {
            sample(i16::from_le_bytes([lo, bytes[0]]));
            bytes = &bytes[1..];
        }
        let mut pairs = bytes.chunks_exact(2);
        for p in &mut pairs {
            sample(i16::from_le_bytes([p[0], p[1]]));
        }
        odd = pairs.remainder().first().copied();
    }
    if n > 0 {
        peaks.push(cur.min(1.0));
    }
    (peaks, n)
}

/// This clip's folder for editor files (`edit-<id>` in the temp folder), and
/// the clip's modification time, against which cached files are judged fresh.
fn edit_dir(src: &ClipMeta) -> Result<(PathBuf, SystemTime), String> {
    let src_time = fs::metadata(&src.path).and_then(|m| m.modified()).map_err(|_| "The clip file is missing".to_string())?;
    let tmp = util::tmp_dir();
    let name = format!("edit-{}", util::sanitize_filename(&src.id));
    remove_stale_dirs(&tmp, &name);
    let dir = tmp.join(&name);
    fs::create_dir_all(&dir).map_err(|e| format!("Cannot create the editor's temp folder: {e}"))?;
    Ok((dir, src_time))
}

fn fresh(p: &Path, since: SystemTime) -> bool {
    fs::metadata(p).is_ok_and(|m| m.len() > 0 && m.modified().is_ok_and(|t| t >= since))
}

fn clip_duration(ffmpeg: &Path, src: &ClipMeta) -> f64 {
    if src.duration > 0.0 { src.duration } else { library::probe(ffmpeg, Path::new(&src.path), false).map(|p| p.duration).unwrap_or(0.0) }
}

/// Waveforms of every audio stream of `src` (cached per clip next to the extracted tracks).
pub fn waveforms(s: &Settings, src: &ClipMeta) -> Result<Vec<Waveform>, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let (dir, src_time) = edit_dir(src)?;
    let cache = dir.join("waveforms.json");
    if fresh(&cache, src_time) {
        if let Ok(w) = fs::read(&cache).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<Vec<Waveform>>(&b).map_err(|e| e.to_string())) {
            return Ok(w);
        }
    }
    let src_path = Path::new(&src.path);
    let n = audio_titles(&ffmpeg, src_path)?.len();
    let per_second = peaks_per_second(clip_duration(&ffmpeg, src));
    let mut waves = Vec::new();
    if n > 0 {
        // The tracks `audio_tracks` extracted are read instead of the clip when
        // they are there: the same audio without gigabytes of video to step over.
        let extracted: Vec<PathBuf> = (0..n).map(|i| dir.join(format!("a{i}.m4a"))).collect();
        let use_extracted = extracted.iter().all(|p| fresh(p, src_time));
        let raw = |i: usize| dir.join(format!("w{i}.part.raw"));
        // One ffmpeg run decodes every stream, each to its own raw file.
        let mut cmd = util::hidden_cmd(&ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
        if use_extracted {
            for p in &extracted {
                cmd.arg("-i").arg(p);
            }
        } else {
            cmd.arg("-i").arg(src_path);
        }
        for i in 0..n {
            let map = if use_extracted { format!("{i}:a:0") } else { format!("0:a:{i}") };
            cmd.args(["-map", &map, "-ac", "1", "-ar", &WAVE_RATE.to_string(), "-c:a", "pcm_s16le", "-f", "s16le"]).arg(raw(i));
        }
        let out = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
        let ok = out.status.success();
        for i in 0..n {
            let peaks = if ok { fs::File::open(raw(i)).map(|f| read_peaks(BufReader::new(f), per_second).0).unwrap_or_default() } else { Vec::new() };
            let _ = fs::remove_file(raw(i));
            waves.push(Waveform { index: i as u32, peaks, per_second });
        }
        if !ok {
            return Err(format!("Could not read the clip's audio: {}", util::last_lines(&out.stderr, 2)));
        }
    }
    // Written under a temporary name first so a failed write never leaves a cache that looks fresh.
    if let Ok(json) = serde_json::to_vec(&waves) {
        let part = dir.join("waveforms.part.json");
        if fs::write(&part, json).is_ok() {
            let _ = fs::rename(&part, &cache);
        }
    }
    Ok(waves)
}

/// The frame to show for each of `count` equal slices of the clip, as
/// (time, exact): exact = the time of a real frame, else just a time to seek
/// to (the first frame after it is shown).
fn thumb_frames(ffmpeg: &Path, file: &Path, duration: f64, count: usize) -> Vec<(f64, bool)> {
    let spacing = duration / count as f64;
    let times: Vec<f64> = (0..count).map(|j| (j as f64 + 0.5) * spacing).collect();
    let mut picks: Vec<Option<f64>> = vec![None; count];
    if let Some(ffprobe) = library::ffprobe_for(ffmpeg) {
        let st = start_time(&ffprobe, file);
        if spacing >= 1.0 {
            // Slices a second or more wide: the keyframe nearest a slice's
            // middle stands for it when it is close by. One decoded frame
            // instead of up to a second of them, and one ffprobe run finds
            // them all (a seek per slice, one packet read after each).
            let tol = (spacing / 4.0).min(0.5);
            let iv: Vec<String> = times.iter().map(|t| format!("{:.6}%+#1", st + t)).collect();
            if let Some(keys) = video_packets(&ffprobe, file, st, &iv.join(",")).filter(|k| k.len() == count) {
                for (j, &(k, key)) in keys.iter().enumerate() {
                    if key && (k - times[j]).abs() <= tol {
                        picks[j] = Some(k);
                    }
                }
            }
            // The rest (a still screen around the middle): the frame on screen there.
            let rest: Vec<usize> = (0..count).filter(|&j| picks[j].is_none()).collect();
            let found = frames_on_screen(ffmpeg, file, &rest.iter().map(|&j| times[j]).collect::<Vec<_>>());
            for (&j, f) in rest.iter().zip(found) {
                picks[j] = f;
            }
        } else if let Some(mut all) = video_packets(&ffprobe, file, st, &format!("{:.6}%{:.6}", st, st + duration + 1.0)) {
            // A short clip (under `count` seconds): every frame time in one
            // read, and the frame on screen for each slice worked out here.
            all.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (j, &t) in times.iter().enumerate() {
                let n = all.partition_point(|p| p.0 <= t + 1e-4);
                picks[j] = n.checked_sub(1).map(|i| all[i].0);
            }
        }
    }
    times.iter().zip(picks).map(|(&t, p)| p.map_or((t, false), |f| (f, true))).collect()
}

/// Film-strip frames that one decoder pass produces.
struct ThumbGroup {
    seek: f64,
    /// Exact frame times, ascending (or a single seek time when not exact).
    frames: Vec<f64>,
    /// (slice, index into `frames`).
    slots: Vec<(usize, usize)>,
    exact: bool,
}

/// Slices that show the same frame share it; on a short clip (`wide` false)
/// frames less than a second apart share one decoder pass, so no stretch of
/// the clip is decoded twice. Wide slices each get their own seek.
fn thumb_groups(picks: &[(f64, bool)], wide: bool) -> Vec<ThumbGroup> {
    let mut groups: Vec<ThumbGroup> = Vec::new();
    for (j, &(t, exact)) in picks.iter().enumerate() {
        if exact {
            if let Some(g) = groups.last_mut().filter(|g| g.exact) {
                let last = g.frames.last().copied().unwrap_or(f64::NEG_INFINITY);
                if (t - last).abs() < 1e-4 {
                    g.slots.push((j, g.frames.len() - 1));
                    continue;
                }
                if !wide && t > last && t - last < 1.0 {
                    g.frames.push(t);
                    g.slots.push((j, g.frames.len() - 1));
                    continue;
                }
            }
            // Half a millisecond early so rounding never skips the frame itself.
            groups.push(ThumbGroup { seek: (t - 0.0005).max(0.0), frames: vec![t], slots: vec![(j, 0)], exact: true });
        } else {
            groups.push(ThumbGroup { seek: t, frames: vec![t], slots: vec![(j, 0)], exact: false });
        }
    }
    groups
}

/// `count` small frames taken evenly across the clip, for the timeline's film
/// strip. Returns their file paths in time order (cached per clip).
pub fn thumbs(s: &Settings, src: &ClipMeta, count: u32) -> Result<Vec<String>, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let count = count.clamp(4, 60) as usize;
    let (dir, src_time) = edit_dir(src)?;
    let final_dir = dir.join(format!("thumbs-{count}"));
    let name = |j: usize| format!("t{j:02}.jpg");
    let list = |d: &Path| (0..count).map(|j| d.join(name(j)).to_string_lossy().into_owned()).collect::<Vec<_>>();
    if (0..count).all(|j| fresh(&final_dir.join(name(j)), src_time)) {
        return Ok(list(&final_dir));
    }
    let duration = clip_duration(&ffmpeg, src);
    if duration <= 0.0 {
        return Err("Could not read the clip's length".into());
    }
    let part = dir.join(format!("thumbs-{count}.part"));
    let _ = fs::remove_dir_all(&part);
    fs::create_dir_all(&part).map_err(|e| format!("Cannot create the editor's temp folder: {e}"))?;

    let src_path = Path::new(&src.path);
    let picks = thumb_frames(&ffmpeg, src_path, duration, count);
    let groups = thumb_groups(&picks, duration / count as f64 >= 1.0);

    // Several groups per ffmpeg run (each an input of its own, decoded side by
    // side), two runs at a time: quick, without taking every core from a game.
    const PER_RUN: usize = 15;
    let batches: Vec<&[ThumbGroup]> = groups.chunks(PER_RUN).collect();
    let group_file = |g: usize, i: usize| part.join(format!("g{g:02}_{:03}.jpg", i + 1));
    let run = |first: usize, batch: &[ThumbGroup]| {
        let mut cmd = util::hidden_cmd(&ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"]);
        for g in batch {
            if g.frames.len() == 1 {
                // One frame: a frame-threaded decoder would decode a dozen more past it.
                cmd.args(["-threads", "1"]);
            } else if let Some(last) = g.frames.last() {
                cmd.args(["-t", &format!("{:.6}", last - g.seek + 0.05)]);
            }
            cmd.args(["-ss", &format!("{:.6}", g.seek), "-i"]).arg(src_path);
        }
        for (k, g) in batch.iter().enumerate() {
            let gi = first + k;
            cmd.args(["-map", &format!("{k}:v:0"), "-frames:v", &g.frames.len().to_string()]);
            if g.frames.len() == 1 {
                // The seek lands on the wanted frame: the first one decoded is it.
                cmd.args(["-vf", "scale=-2:72:flags=area", "-update", "1"]).arg(group_file(gi, 0));
            } else {
                // Timestamps count from the seek point; frames are at least 4 ms apart.
                let pick: Vec<String> = g.frames.iter().map(|f| format!("lt(abs(t-{:.6}),0.0004)", f - g.seek)).collect();
                cmd.args(["-vf", &format!("select='{}',scale=-2:72:flags=area", pick.join("+")), "-fps_mode", "passthrough"]).arg(part.join(format!("g{gi:02}_%03d.jpg")));
            }
            cmd.args(["-q:v", "4"]);
        }
        let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    let run = &run;
    std::thread::scope(|sc| {
        for (p, pair) in batches.chunks(2).enumerate() {
            let handles: Vec<_> = pair.iter().enumerate().map(|(b, batch)| sc.spawn(move || run((p * 2 + b) * PER_RUN, batch))).collect();
            for h in handles {
                let _ = h.join();
            }
        }
    });
    for (gi, g) in groups.iter().enumerate() {
        for &(j, i) in &g.slots {
            let _ = fs::copy(group_file(gi, i), part.join(name(j)));
        }
        for i in 0..g.frames.len() {
            let _ = fs::remove_file(group_file(gi, i));
        }
    }

    // A frame that could not be read (past the last frame, a damaged spot) is
    // replaced by its nearest neighbour rather than failing the whole strip.
    let have: Vec<bool> = (0..count).map(|j| fs::metadata(part.join(name(j))).is_ok_and(|m| m.len() > 0)).collect();
    if !have.iter().any(|&h| h) {
        let _ = fs::remove_dir_all(&part);
        return Err("Could not read any frames from the clip".into());
    }
    for j in (0..count).filter(|&j| !have[j]) {
        let earlier = (0..j).rev().find(|&i| have[i]);
        let near = earlier.or_else(|| (j + 1..count).find(|&i| have[i]));
        if let Some(i) = near {
            let _ = fs::copy(part.join(name(i)), part.join(name(j)));
        }
    }
    let _ = fs::remove_dir_all(&final_dir);
    fs::rename(&part, &final_dir).map_err(|e| format!("Cannot prepare the film strip: {e}"))?;
    Ok(list(&final_dir))
}

/// Check that `path` is a file with audio in it and describe it for the timeline.
pub fn audio_info(s: &Settings, path: &str) -> Result<AudioInfo, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let file = Path::new(path.trim());
    if !file.is_file() {
        return Err("That file does not exist (was it moved or deleted?)".into());
    }
    let probe = probe_audio(&ffmpeg, file).map_err(|_| "That file could not be read. It may be damaged, or not a sound or video file.".to_string())?;
    if probe.channels.is_empty() {
        return Err("That file has no audio in it".into());
    }
    let per_second = peaks_per_second(probe.duration);
    let mut child = util::hidden_cmd(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", "0:a:0", "-ac", "1", "-ar", &WAVE_RATE.to_string(), "-c:a", "pcm_s16le", "-f", "s16le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let (peaks, samples) = child.stdout.take().map(|out| read_peaks(out, per_second)).unwrap_or_default();
    let ok = child.wait().is_ok_and(|st| st.success());
    if !ok && samples == 0 {
        return Err("The audio in that file could not be read".into());
    }
    // Some formats (raw streams, a few broken files) have no stated length: the decoded audio has.
    let duration = if probe.duration > 0.0 { probe.duration } else { samples as f64 / WAVE_RATE as f64 };
    Ok(AudioInfo {
        path: file.to_string_lossy().into_owned(),
        title: file.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        duration,
        peaks,
        per_second,
    })
}
