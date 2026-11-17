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

// Pictures, GIFs and clips on the picture; round / soft blur boxes (0.1.35).
#[path = "edmedia.rs"]
mod edmedia;
// Smart rendering of cuts-only exports (0.1.46).
#[path = "smartcut.rs"]
mod smartcut;
pub use edmedia::{media_info, MediaInfo, MediaSrc, MEDIA_EXT};

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
    /// 0.1.41: sharpening, 0 = off .. 1: `unsharp` with a 5x5 luma matrix, amount 1.5 x this (chroma untouched).
    pub sharpness: f32,
    /// 0.1.45: -1 (cool) .. +1 (warm), 0 = off: R x (1 + 0.20 T), G x (1 + 0.02 T), B x (1 - 0.20 T), after the
    /// brightness / contrast / saturation / gamma step (the preview uses the same matrix).
    pub temperature: f32,
    /// 0.1.45: 0 .. 1, 0 = off: a bloom: the highlights (each channel through max(0, (x - 0.55) / 0.45)), blurred
    /// (Gaussian, sigma 1.6 % of the picture's height), screened back over the picture at opacity 0.9 x glow.
    pub glow: f32,
}

impl Default for Color {
    fn default() -> Self {
        Color { brightness: 1.0, contrast: 1.0, saturation: 1.0, gamma: 1.0, sharpness: 0.0, temperature: 0.0, glow: 0.0 }
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
    /// 0.1.36: > 0 = this entry is not a range but a GAP on the main video row: that many
    /// timeline seconds of black picture and silence from the clip's tracks, in its place in
    /// `segments` (`start`, `end`, `color`, `blank` are ignored then).
    pub gap: f64,
    /// 0.1.41: this range plays at this speed (picture and its tracks' sound, pitch kept); 0 or 1 = as it was.
    /// `start` / `end` stay source seconds: the range lasts (end - start) / speed on the timeline.
    pub speed: f64,
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
    /// File rows (0.1.39): which audio stream of `path` to mix (0 = the first). `path` may be a video
    /// file with several (an ungrouped piece's sound is a track of its own clip's file).
    pub stream: u32,
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
    /// 0.1.35: OUTPUT seconds of black after everything kept ("after the video": the timeline
    /// ends where its last overlay item or audio file ends). The picture is black there, at the
    /// clip's frame rate, so overlays and keyframes go on; every audio result is padded with
    /// silence to the same length; files that play through cuts may sound there. 0 = none.
    pub tail: f64,
    /// 0.1.38, a project that references its clips (`projexport.rs`): the timeline's audio rows by title,
    /// one per title across the sources; a track row's `index` is its place in this list. Empty = a clip's own
    /// stream index, as before. `editor::export` itself never reads it.
    pub track_titles: Vec<String>,
    /// 0.1.44, audio rows by role (CONTRACT_0144R): per source, the audio stream each track row plays
    /// (`streams[k]` for the row whose `index` is k; -1 = none). Present = it decides, not the titles.
    pub track_map: Vec<TrackMap>,
    /// 0.1.44: a piece whose row plays another stream of its source than `track_map` says (the "only the mix
    /// was audible" case of the conversion): row `row`, from `start` to `end` (source seconds, as `segments`).
    pub track_over: Vec<TrackOver>,
}

/// One source's streams per track row (`ExportOpts::track_map`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TrackMap {
    pub clip: String,
    pub streams: Vec<i64>,
}

/// A stretch of a track row that plays `stream` instead (`ExportOpts::track_over`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct TrackOver {
    pub row: u32,
    pub start: f64,
    pub end: f64,
    pub stream: u32,
}

/// Audio rows by role for ONE file (0.1.44): `map` = that file's entry of `track_map` (track row k -> stream,
/// -1 none) and the overrides, as plain stream rows: each track row then names the stream it plays, a row with
/// none is dropped, and an override is the row's level on the other stream for that stretch (and silence on its
/// own stream there). Without a map and overrides the options are returned as they are.
pub fn rows_by_stream(opts: &ExportOpts, map: Option<&TrackMap>) -> ExportOpts {
    let mut o = opts.clone();
    o.track_map = Vec::new();
    o.track_over = Vec::new();
    if map.is_none() && opts.track_over.is_empty() {
        return o;
    }
    let stream_of = |k: u32| -> Option<u32> {
        match map {
            Some(m) => m.streams.get(k as usize).copied().filter(|s| *s >= 0).map(|s| s as u32),
            None => Some(k),
        }
    };
    let mut audio = Vec::new();
    for e in &opts.audio {
        if e.source != "track" {
            audio.push(e.clone());
            continue;
        }
        let overs: Vec<&TrackOver> = opts.track_over.iter().filter(|v| v.row == e.index && v.end > v.start).collect();
        if let Some(s) = stream_of(e.index) {
            // Silent where an override plays another stream (first listed wins).
            let mut rs: Vec<GainRegion> = overs.iter().map(|v| GainRegion { start: v.start, end: v.end, gain: 0.0 }).collect();
            rs.extend(e.regions.iter().cloned());
            audio.push(AudioEdit { index: s, regions: rs, ..e.clone() });
        }
        for v in overs {
            // The other stream, heard only from start to end, at the row's level.
            let mut rs = vec![GainRegion { start: -1.0e6, end: v.start, gain: 0.0 }, GainRegion { start: v.end, end: 1.0e6, gain: 0.0 }];
            rs.extend(e.regions.iter().cloned());
            audio.push(AudioEdit { index: v.stream, regions: rs, ..e.clone() });
        }
    }
    if audio.is_empty() && !opts.audio.is_empty() {
        // Every row had nothing to play: nothing heard (not "the clip as recorded").
        o.tracks = vec![TrackVolume { index: 0, volume: 0.0 }];
    }
    o.audio = audio;
    if map.is_some() {
        o.tracks = o.tracks.iter().filter_map(|t| Some(TrackVolume { index: if t.volume <= 0.0 { t.index } else { stream_of(t.index)? }, volume: t.volume })).collect();
    }
    o
}

impl Default for ExportOpts {
    fn default() -> Self {
        ExportOpts { start: 0.0, end: 0.0, speed: 1.0, color: Color::default(), tracks: Vec::new(), segments: Vec::new(), audio: Vec::new(), ordered: false, overlays: Vec::new(), blurs: Vec::new(), fade_in: 0.0, fade_out: 0.0, canvas: None, canvas_tracks: Vec::new(), tail: 0.0, track_titles: Vec::new(), track_map: Vec::new(), track_over: Vec::new() }
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
    /// A picture, GIF or clip (0.1.35, edmedia.rs) instead of `png`: shown
    /// in the box `x`, `y`, `w`, `h` (also without a track).
    pub media: Option<MediaSrc>,
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
    /// 0.1.35: corners rounded 0..1 (1 = an ellipse) and the blur fading out
    /// toward the edge 0..1 (edmedia::mask_alpha). 0 and 0 = a plain box.
    pub round: f64,
    pub soft: f64,
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
    [c.brightness, c.contrast, c.saturation, c.gamma].iter().all(|v| !v.is_finite() || same(*v as f64, 1.0)) && sharp_amount(c) == 0.0 && temp_amount(c) == 0.0 && glow_amount(c) == 0.0
}

/// `Color::temperature` kept to -1..1 (0 = off).
fn temp_amount(c: &Color) -> f64 {
    if c.temperature.is_finite() && (c.temperature as f64).abs() > 0.001 { (c.temperature as f64).clamp(-1.0, 1.0) } else { 0.0 }
}

/// `Color::glow` kept to 0..1 (0 = off).
fn glow_amount(c: &Color) -> f64 {
    if c.glow.is_finite() && c.glow > 0.001 { (c.glow as f64).min(1.0) } else { 0.0 }
}

/// The glow's highlight knee, blur (a fraction of the picture's height) and strength (opacity per unit of glow).
const GLOW_KNEE: f64 = 0.55;
const GLOW_SIGMA: f64 = 0.016;
const GLOW_OPACITY: f64 = 0.9;
/// The highlights are blurred on a copy this many pixels high (sigma scaled with it) and scaled back: the same
/// light as at full size, a fraction of the work on 1440p.
const GLOW_WORK_H: u32 = 360;
/// Labels of the glow's branches: one set per use in a graph.
static GLOW_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The `unsharp` luma amount of `c` (0 = none): 1.5 x sharpness, sharpness kept to 0..1.
fn sharp_amount(c: &Color) -> f64 {
    if c.sharpness.is_finite() && c.sharpness > 0.001 { 1.5 * (c.sharpness as f64).min(1.0) } else { 0.0 }
}

/// True if `a` and `b` give the same picture (a non-finite field counts as 1, as in `is_neutral`).
fn same_color(a: &Color, b: &Color) -> bool {
    let f = |v: f32| if v.is_finite() { v as f64 } else { 1.0 };
    [(a.brightness, b.brightness), (a.contrast, b.contrast), (a.saturation, b.saturation), (a.gamma, b.gamma)].iter().all(|&(x, y)| same(f(x), f(y))) && same(sharp_amount(a), sharp_amount(b)) && same(temp_amount(a), temp_amount(b)) && same(glow_amount(a), glow_amount(b))
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

    let titles = crate::edcache::titles(src_path, || audio_titles(&ffmpeg, src_path))?; // P-HOOK 0.1.41 (edcache.rs): remembered per file
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
        let _slot = crate::edcache::slot_light()?; // P-HOOK 0.1.41 (edcache.rs): a few of these at once, cancellable
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
    // Sharpness alone (0.1.41): only the unsharp, no trip through RGB.
    let sharp = sharp_amount(c);
    let unsharp = format!("unsharp=luma_msize_x=5:luma_msize_y=5:luma_amount={sharp:.4}:chroma_msize_x=5:chroma_msize_y=5:chroma_amount=0");
    let (temp, glow) = (temp_amount(c), glow_amount(c));
    if glow > 0.0 {
        // 0.1.45: the look without its glow, then the glow (order: eq, temperature, sharpness, glow).
        let before = color_filter(&Color { glow: 0.0, ..c.clone() }, matrix, range);
        let n = GLOW_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (a, b, h, s, o) = (format!("gwa{n}"), format!("gwb{n}"), format!("gwh{n}"), format!("gws{n}"), format!("gwo{n}"));
        let k = 255.0 * GLOW_KNEE;
        let hi = format!("max(0,(val-{k:.3})/{:.4})", 1.0 - GLOW_KNEE);
        let sigma = GLOW_SIGMA * GLOW_WORK_H as f64;
        let glow_part = format!(
            "scale=in_color_matrix={matrix}:in_range={range}:flags=bicubic+accurate_rnd+full_chroma_int,format=gbrp,split=2[{a}][{b}];[{b}]lutrgb=r='{hi}':g='{hi}':b='{hi}',scale=-2:{GLOW_WORK_H},gblur=sigma={sigma:.3}[{h}];[{h}][{a}]scale2ref=flags=bicubic[{s}][{o}];[{o}][{s}]blend=all_mode=screen:all_opacity={:.4},scale=out_color_matrix={matrix}:out_range={range}:flags=bicubic+accurate_rnd",
            GLOW_OPACITY * glow
        );
        return Some(match before {
            Some(f) => format!("{f},{glow_part}"),
            None => glow_part,
        });
    }
    if [c.brightness, c.contrast, c.saturation, c.gamma].iter().all(|v| !v.is_finite() || same(*v as f64, 1.0)) && temp == 0.0 {
        return Some(unsharp);
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
        // No channel mixing: brightness, contrast and gamma fold into one table (none when they change nothing:
        // a temperature alone, 0.1.45).
        if curve != "val" || !same(g, 1.0) {
            steps.push(lut(&if same(g, 1.0) { curve } else { gamma(&curve) }));
        }
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
    if temp != 0.0 {
        // 0.1.45: warm / cool, the same matrix as the preview's.
        steps.push(format!("colorchannelmixer=rr={:.5}:gg={:.5}:bb={:.5}", 1.0 + 0.20 * temp, 1.0 + 0.02 * temp, 1.0 - 0.20 * temp));
    }
    steps.push(format!("scale=out_color_matrix={matrix}:out_range={range}:flags=bicubic+accurate_rnd"));
    if sharp > 0.0 {
        steps.push(unsharp);
    }
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

/// Seconds without a new frame (no advance of `out_time`) after which an export is ended as stalled (0.1.41).
pub const STALL_SECS: u64 = 60;
/// Tests: a shorter stall limit in milliseconds (0 = `STALL_SECS`).
static STALL_MS_TEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// What an export that was cancelled fails with (the UI shows "Export cancelled" for it).
pub const CANCELLED: &str = "Export cancelled";

/// Running exports by the id the UI knows them by: `cancel_export` sets the flag (0.1.41).
static RUNNING: Mutex<Vec<(String, std::sync::Arc<std::sync::atomic::AtomicBool>)>> = Mutex::new(Vec::new());

/// Holds an export's entry in `RUNNING` for as long as it runs.
pub struct ExportGuard {
    id: String,
    pub flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ExportGuard {
    pub fn new(id: &str) -> ExportGuard {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        RUNNING.lock().unwrap().push((id.to_string(), flag.clone()));
        ExportGuard { id: id.to_string(), flag }
    }
}

impl Drop for ExportGuard {
    fn drop(&mut self) {
        let mut list = RUNNING.lock().unwrap();
        if let Some(i) = list.iter().position(|(id, f)| *id == self.id && std::sync::Arc::ptr_eq(f, &self.flag)) {
            list.remove(i);
        }
    }
}

/// End the running export `id`: true when there was one (it then fails with `CANCELLED`).
pub fn cancel_export(id: &str) -> bool {
    let list = RUNNING.lock().unwrap();
    let mut any = false;
    for (x, f) in list.iter().filter(|(x, _)| x == id) {
        let _ = x;
        f.store(true, std::sync::atomic::Ordering::SeqCst);
        any = true;
    }
    any
}

thread_local! {
    /// The cancel flag of the export this thread runs (set by `export_watched`), read by `run_with_progress`.
    static CANCEL: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

/// Run ffmpeg with `-progress pipe:1` and report 0..100 against `expected` seconds of output.
/// 0.1.41: the child is tied to Glint's life (`util::track_child`); when its `out_time` does not advance
/// for `STALL_SECS` it is ended and the export fails saying where it stopped; a cancel ends it too.
fn run_with_progress(ffmpeg: &Path, args: &[String], expected: f64, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(args);
    let cancel = CANCEL.with(|c| c.borrow().clone());
    run_watched(cmd, expected, progress, cancel.as_deref())
}

/// `run_with_progress` for any prepared command (tests give it a child that goes silent).
fn run_watched(mut cmd: std::process::Command, expected: f64, progress: &mut dyn FnMut(f32), cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    util::track_child(&child);
    // Drained on its own thread so a chatty stderr can never block ffmpeg while we read progress.
    let err_reader = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let stall = match STALL_MS_TEST.load(Ordering::Relaxed) {
        0 => Duration::from_secs(STALL_SECS),
        ms => Duration::from_millis(ms),
    };
    let mut last = -1.0f32;
    let mut out_us = -1.0f64;
    let mut moved = Instant::now();
    let mut stopped: Option<String> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                    if us > out_us {
                        out_us = us;
                        moved = Instant::now();
                    }
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
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            stopped = Some(CANCELLED.to_string());
        } else if last >= 40.0 && HW_RUN.with(|h| h.get()) && HW_TEST.with(|t| t.get()) == 2 {
            // Test (0.1.46): the graphics card's decoding stops part way.
            stopped = Some("test: the hardware decoder stopped at 40 %".to_string());
        } else if moved.elapsed() >= stall {
            let pct = if expected > 0.0 { ((out_us.max(0.0) / 1e6 / expected) * 100.0).clamp(0.0, 99.0).floor() } else { 0.0 };
            stopped = Some(format!("The export stopped making progress at {pct:.0} %. ffmpeg made no new frame for {} seconds, so it was ended. The log has its last words.", stall.as_secs().max(1)));
        }
        if stopped.is_some() {
            let _ = child.kill();
            break;
        }
    }
    let status = child.wait().map_err(|e| format!("ffmpeg failed: {e}"))?;
    LAST_CPU.with(|l| l.set(util::child_cpu_secs(&child).unwrap_or(f64::NAN)));
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if let Some(why) = stopped {
        if why.starts_with("The export stopped") {
            util::log(&format!("export: stalled at {:.1} s of output; ffmpeg said: {}", out_us.max(0.0) / 1e6, util::last_lines(&stderr, 4)));
        }
        return Err(why);
    }
    if status.success() {
        Ok(())
    } else {
        let tail = util::last_lines(&stderr, 2);
        Err(if tail.is_empty() { format!("ffmpeg stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail })
    }
}
thread_local! {
    /// Test hook: Some = `export` stops before ffmpeg and leaves its command line here.
    static DRY_RUN: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
    /// Test hook: Some = the command-line length the range inputs must fit in (0.1.42), instead of 32000.
    pub(crate) static GROUP_BUDGET: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// Test hook: true = cuts stay where they were asked (0.1.43 and before), for the checks that compare.
    pub(crate) static NO_SNAP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test hook: true = a moving overlay that only moves is a looped picture as before 0.1.44 (to compare frames).
    pub(crate) static STILL_AS_LOOP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test hook: how many range inputs the last `export` used.
    pub(crate) static LAST_INPUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The ffmpeg command `export` would run for `opts` (libx264; paths as `<src>`, `<out>`, `<tmp>`, `<data>`).
fn export_args(s: &Settings, src: &ClipMeta, opts: &ExportOpts) -> Result<Vec<String>, String> {
    DRY_RUN.with(|d| *d.borrow_mut() = Some(Vec::new()));
    let res = export(s, src, opts, &mut |_| {});
    let args = DRY_RUN.with(|d| d.borrow_mut().take()).unwrap_or_default();
    match res {
        Err(e) if e == "dry run" => Ok(args),
        Err(e) => Err(e),
        Ok(m) => Err(format!("no command: {} (a lossless cut?)", m.kind)),
    }
}

/// FNV-1a over the arguments (each ended by a NUL), as 16 hex digits.
fn args_hash(args: &[String]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for a in args {
        for b in a.bytes().chain(std::iter::once(0u8)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
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
    /// Timeline seconds of gap (black, silent) just before this range in the result (0.1.36).
    gap: f64,
    /// 0.1.41: the range's own speed (1 = none).
    speed: f64,
}

impl Kept {
    fn looks_like(&self, o: &Kept) -> bool {
        self.blank == o.blank && same_color(&self.color, &o.color) && same(self.speed, o.speed)
    }
}

/// A range's own speed (0.1.41): 0.25 ..= 4, absent / 0 / not a number = 1.
fn range_speed(g: &Range) -> f64 {
    if g.speed.is_finite() && g.speed > 0.0 { g.speed.clamp(0.25, 4.0) } else { 1.0 }
}

/// A `segments` entry that is a gap (0.1.36).
fn is_gap(g: &Range) -> bool {
    g.gap.is_finite() && g.gap > 0.0005
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
        return vec![Kept { a: start, b: if to_end { duration } else { opts.end }, color: opts.color.clone(), blank: false, gap: 0.0, speed: 1.0 }];
    }
    if opts.segments.iter().any(is_gap) {
        // Gaps (0.1.36): in the order given; each gap goes to the range after it, and two
        // ranges with a gap between them never join. A gap after the last range is dropped
        // (the model never sends one: `tail` is the time after the video).
        let mut out: Vec<Kept> = Vec::new();
        let mut pending = 0.0;
        for g in &opts.segments {
            if is_gap(g) {
                pending += g.gap.min(3600.0);
                continue;
            }
            if !(g.start.is_finite() && g.end.is_finite()) {
                continue;
            }
            let mut k = Kept {
                a: g.start.clamp(0.0, duration),
                b: g.end.clamp(0.0, duration),
                color: if g.blank { Color::default() } else { g.color.clone().unwrap_or_else(|| opts.color.clone()) },
                blank: g.blank,
                gap: 0.0,
                speed: range_speed(g),
            };
            if k.b - k.a <= 0.001 {
                continue;
            }
            if pending == 0.0 {
                if let Some(last) = out.last_mut() {
                    if (k.a - last.b).abs() <= 1e-3 {
                        if k.looks_like(last) {
                            last.b = k.b;
                            continue;
                        }
                        k.a = last.b;
                    }
                }
            }
            k.gap = pending;
            pending = 0.0;
            out.push(k);
        }
        return out;
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
            gap: 0.0,
            speed: range_speed(g),
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
    File { path: String, seek: f64, len: f64, at: f64, channels: u32, stream: u32 },
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
                // The stream it names must be there (a clip's own file has one per track).
                let Some(&channels) = info.channels.get(e.stream as usize) else {
                    let name = Path::new(&e.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| e.path.clone());
                    return Err(format!("The file \"{name}\" has no audio track {} (it has {})", e.stream + 1, info.channels.len()));
                };
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
                    rows.push(MixRow { source: Source::File { path: e.path.clone(), seek, len, at, channels, stream: e.stream }, volume, regions, through: true });
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
                rows.push(MixRow { source: Source::File { path: e.path.clone(), seek, len, at, channels, stream: e.stream }, volume, regions, through: false });
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

/// `cut_chain` with gaps (0.1.36): `gaps[i]` seconds of silence (`silence`: an `anullsrc` of the
/// input's rate and layout) go just before range `i`, each exactly that many samples long.
/// Without any gap this is `cut_chain` itself, character for character.
fn cut_chain_gaps(input: &str, ranges: &[(f64, Option<f64>)], gaps: &[f64], speeds: &[f64], tag: &str, tempo: &str, out: &str, silence: (u32, &str)) -> String {
    if gaps.iter().all(|g| *g <= 0.0) && speeds.iter().all(|x| same(*x, 1.0)) {
        return cut_chain(input, ranges, tag, tempo, out);
    }
    let k = ranges.len();
    let (rate, layout) = silence;
    let mut c = format!("{input}asplit={k}");
    for i in 0..k {
        c += &format!("[{tag}c{i}]");
    }
    let mut parts = String::new();
    let mut n = 0;
    for (i, (a, b)) in ranges.iter().enumerate() {
        let g = gaps.get(i).copied().unwrap_or(0.0);
        if g > 0.0 {
            let samples = (g * rate as f64).round() as u64;
            c += &format!(";anullsrc=r={rate}:cl={layout},atrim=end_sample={samples}[{tag}g{i}]");
            parts += &format!("[{tag}g{i}]");
            n += 1;
        }
        let end = b.map(|b| format!(":end={b:.6}")).unwrap_or_default();
        // A range with its own speed (0.1.41): its sound at that speed, the pitch kept.
        let sk = speeds.get(i).copied().unwrap_or(1.0);
        // `atempo` ends a short piece early (a 4-frame piece at 0.5x lost 56 ms: 4 s over a 150-piece montage, 0.1.44),
        // so a sped piece is padded with silence and cut to exactly its length on the timeline.
        let own = match (same(sk, 1.0), b) {
            (true, _) => String::new(),
            (false, Some(b)) => { let len = (b - a) / sk; format!(",{},apad=whole_dur={len:.6},atrim=end={len:.6}", atempo_chain(sk)) }
            (false, None) => format!(",{}", atempo_chain(sk)),
        };
        c += &format!(";[{tag}c{i}]atrim=start={a:.6}{end},asetpts=PTS-STARTPTS{own}[{tag}s{i}]");
        parts += &format!("[{tag}s{i}]");
        n += 1;
    }
    c + &format!(";{parts}concat=n={n}:v=0:a=1,{tempo}{out}")
}

/// A row's level: its volume, times each region's gain inside that region.
fn gain_filter(volume: f64, regions: &[(f64, f64, f64)]) -> String {
    if regions.is_empty() {
        return if same(volume, 1.0) { "anull".into() } else { format!("volume={volume:.4}") };
    }
    // The first region listed wins where two overlap. Up to 0.1.40 one nested `if` per region: ffmpeg's
    // expression parser gives up at about 100 levels (R3-1). Now the regions are made disjoint first and
    // summed: 1 + (g - 1) over each, which nests nothing however many there are.
    let mut e = "1".to_string();
    if regions.len() <= 8 {
        for (a, b, g) in regions.iter().rev() {
            e = format!("if(gte(t,{a:.4})*lt(t,{b:.4}),{g:.4},{e})");
        }
    } else {
        let mut taken: Vec<(f64, f64, f64)> = Vec::new();
        for &(a, b, g) in regions {
            // The parts of a..b that no earlier region holds.
            let mut parts = vec![(a, b)];
            for &(x, y, _) in &taken {
                parts = parts.into_iter().flat_map(|(p, q)| {
                    let mut out = Vec::new();
                    if p < x.min(q) { out.push((p, x.min(q))); }
                    if q > y.max(p) { out.push((y.max(p), q)); }
                    out
                }).filter(|(p, q)| q - p > 1e-6).collect();
            }
            taken.extend(parts.into_iter().map(|(p, q)| (p, q, g)));
        }
        for (a, b, g) in taken.iter().filter(|r| (r.2 - 1.0).abs() > 1e-9) {
            e += &format!("+{:.4}*gte(t,{a:.4})*lt(t,{b:.4})", g - 1.0);
        }
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

/// For each of `times`: the frame times next to it in `file`, (the last at or before it, the first after it),
/// from packet times within a quarter second after and up to 2 s before (None where there is none).
fn frames_around(ffmpeg: &Path, file: &Path, times: &[f64]) -> Vec<(Option<f64>, Option<f64>)> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return vec![(None, None); times.len()] };
    let start = start_time(&ffprobe, file);
    let one = |t: f64| -> (Option<f64>, Option<f64>) {
        let Some(packets) = video_packets(&ffprobe, file, start, &format!("{:.6}%{:.6}", start + (t - 2.0).max(0.0), start + t + 0.25)) else { return (None, None) };
        let before = packets.iter().map(|p| p.0).filter(|p| *p <= t + 1e-4).reduce(f64::max);
        let after = packets.iter().map(|p| p.0).filter(|p| *p > t + 1e-4).reduce(f64::min);
        (before, after)
    };
    let one = &one;
    let mut found = vec![(None, None); times.len()];
    std::thread::scope(|sc| {
        for (ts, slots) in times.chunks(8).zip(found.chunks_mut(8)) {
            let runs: Vec<_> = ts.iter().map(|&t| sc.spawn(move || one(t))).collect();
            for (slot, run) in slots.iter_mut().zip(runs) {
                *slot = run.join().unwrap_or((None, None));
            }
        }
    });
    found
}

/// 0.1.44, frame-exact cuts: every range start and end moved to the nearest frame time of the source (picture and
/// sound together, since both are cut from these numbers), at most half a frame. A cut between two frames showed
/// the frame on screen there for less than a frame (or the speed cap dropped it); now each range is whole frames,
/// so across every cut the frames are exactly one frame apart. A point with no frame within a frame interval (a
/// still screen in a variable-rate clip) and the clip's own ends stay where they are. Touching ranges move together.
fn snap_to_frames(ffmpeg: &Path, file: &Path, pieces: &mut [Kept], fps: u32, duration: f64) {
    if pieces.is_empty() || fps == 0 {
        return;
    }
    let frame = 1.0 / fps as f64;
    let mut times: Vec<f64> = pieces.iter().flat_map(|k| [k.a, k.b]).filter(|t| *t > 1e-4 && *t < duration - 0.002).collect();
    times.sort_by(|x, y| x.total_cmp(y));
    times.dedup_by(|x, y| (*x - *y).abs() < 1e-6);
    let near = frames_around(ffmpeg, file, &times);
    let snap = |t: f64| -> f64 {
        let Some(i) = times.iter().position(|x| (x - t).abs() < 1e-6) else { return t };
        let (before, after) = near[i];
        let best = [before, after].into_iter().flatten().filter(|f| (f - t).abs() <= frame * 0.5 + 1e-6).min_by(|x, y| (x - t).abs().total_cmp(&(y - t).abs()));
        best.unwrap_or(t)
    };
    for k in pieces.iter_mut() {
        let (a, b) = (snap(k.a), snap(k.b));
        // A range is never snapped away: at least one frame stays.
        if b - a >= frame * 0.5 {
            k.a = a;
            k.b = b;
        }
    }
    // A sped-up range is made a whole number of the RESULT's frames too (at 2x an even number of source frames):
    // half a frame left over made the speed cap drop the next range's first frame. Its end moves to the nearest
    // such frame; a range after it that started where it ended moves with it.
    for i in 0..pieces.len() {
        let sk = pieces[i].speed;
        if sk <= 1.0 + 1e-6 {
            continue;
        }
        let n = ((pieces[i].b - pieces[i].a) / frame).round();
        let whole = ((n / sk).round() * sk).round();
        if whole < 1.0 || (whole - n).abs() < 1e-9 || ((n / sk).round() * sk - whole).abs() > 1e-6 {
            continue;
        }
        let (old, new) = (pieces[i].b, (pieces[i].a + whole * frame).min(duration));
        pieces[i].b = new;
        if let Some(next) = pieces.get_mut(i + 1) {
            if (next.a - old).abs() < 1e-6 && next.b - new >= frame * 0.5 {
                next.a = new;
            }
        }
    }
}

/// The picture layers of an export, checked and clamped to the result's length.
struct Layers {
    /// (png path, from, to) in output seconds, how it moves (keyframes), and
    /// the picture / GIF / clip it shows instead of the PNG (edmedia.rs).
    overlays: Vec<(String, f64, f64, Option<Moving>, Option<edmedia::MediaPlan>)>,
    /// (x, y, w, h fractions, from, to, strength), how it moves, (round, soft).
    blurs: Vec<(f64, f64, f64, f64, f64, f64, f64, Option<Moving>, (f64, f64))>,
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

impl Moving {
    /// Whether its size changes along the track (0.1.44): only then does its picture need a new frame for every
    /// output frame. A track that only moves it is one still picture placed anew each frame by `overlay`.
    fn size_moves(&self) -> bool {
        STILL_AS_LOOP.with(|c| c.get()) || self.pts.iter().any(|p| (p.1[2] - self.pts[0].1[2]).abs() > 1e-9 || (p.1[3] - self.pts[0].1[3]).abs() > 1e-9)
    }
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
        if let Some(ms) = &o.media {
            // A picture, GIF or clip in its box (edmedia.rs); it may hang over the picture's edges.
            if let Some(mp) = edmedia::media_plan(o, ms)? {
                let moving = clean_moves(&o.track, total).map(|pts| Moving { bx: mp.bx, pts });
                overlays.push((String::new(), a, b, moving, Some(mp)));
            }
            continue;
        }
        if !Path::new(&o.png).is_file() {
            return Err("An overlay picture of the edit is missing. Try the export again.".into());
        }
        // A moving one: the PNG is the item's box (it may hang over the picture's edges).
        let moving = clean_moves(&o.track, total).and_then(|pts| {
            let (w, h) = (fin(o.w).min(4.0), fin(o.h).min(4.0));
            (w > 0.0005 && h > 0.0005).then(|| Moving { bx: (fin(o.x).clamp(-4.0, 4.0), fin(o.y).clamp(-4.0, 4.0), w, h), pts })
        });
        overlays.push((o.png.clone(), a, b, moving, None));
    }
    let mut blurs = Vec::new();
    for g in &opts.blurs {
        let Some((a, b)) = span(g.from, g.to) else { continue };
        let st = fin(g.strength).clamp(0.0, 0.2);
        let shape = (fin(g.round).clamp(0.0, 1.0), fin(g.soft).clamp(0.0, 1.0));
        if let Some(pts) = clean_moves(&g.track, total) {
            // A moving box: kept whole (its track may take it partly off the picture).
            let (x, y) = (fin(g.x).clamp(-1.0, 2.0), fin(g.y).clamp(-1.0, 2.0));
            let (w, h) = (fin(g.w).min(1.0), fin(g.h).min(1.0));
            if w > 0.002 && h > 0.002 && st > 0.0 {
                blurs.push((x, y, w, h, a, b, st, Some(Moving { bx: (x, y, w, h), pts }), shape));
            }
            continue;
        }
        let x = fin(g.x).clamp(0.0, 1.0);
        let y = fin(g.y).clamp(0.0, 1.0);
        let w = fin(g.w).min(1.0 - x);
        let h = fin(g.h).min(1.0 - y);
        if w > 0.002 && h > 0.002 && st > 0.0 {
            blurs.push((x, y, w, h, a, b, st, None, shape));
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
/// A media item (edmedia.rs) takes its overlay's place in the order. `masks`
/// (one per blur box, edmedia::write_blur_masks) are the inputs after the
/// overlays'; a box with one has its blurred copy faded by it (`alphamerge`).
fn layer_chain(l: &Layers, first_png: usize, (pw, ph): (u32, u32), total: f64, masks: &[Option<PathBuf>]) -> Vec<String> {
    let mut g = Vec::new();
    let mut cur = "vb".to_string();
    let even = |v: f64| ((v / 2.0).round() as i64 * 2).max(0) as u32;
    let first_mask = first_png + l.overlays.len();
    for (i, &(x, y, w, h, a, b, st, ref moving, _)) in l.blurs.iter().enumerate() {
        let mask = edmedia::mask_input(masks, i, first_mask);
        if let Some(mv) = moving {
            g.extend(moving_blur_chain(mv, &cur, i, (pw, ph), (a, b, st), mask));
            cur = format!("vl{i}");
            continue;
        }
        let bx = even(x * pw as f64).min(pw.saturating_sub(2));
        let by = even(y * ph as f64).min(ph.saturating_sub(2));
        let bw = even(w * pw as f64).clamp(2, pw - bx);
        let bh = even(h * ph as f64).clamp(2, ph - by);
        let sigma = (st * ph as f64).clamp(0.5, 500.0);
        let on = format!("enable='between(t,{a:.4},{b:.4})'");
        if let Some(mk) = mask {
            g.push(format!(
                "[{cur}]split=2[{cur}m][{cur}s];[{cur}s]crop={bw}:{bh}:{bx}:{by},gblur=sigma={sigma:.3}:steps=2:{on},format=yuva420p[bq{i}];[{mk}:v]format=gray,scale={bw}:{bh}[mk{i}];[bq{i}][mk{i}]alphamerge[bb{i}];[{cur}m][bb{i}]overlay={bx}:{by}:{on}[vl{i}]"
            ));
            cur = format!("vl{i}");
            continue;
        }
        g.push(format!(
            "[{cur}]split=2[{cur}m][{cur}s];[{cur}s]crop={bw}:{bh}:{bx}:{by},gblur=sigma={sigma:.3}:steps=2:{on}[bb{i}];[{cur}m][bb{i}]overlay={bx}:{by}:{on}[vl{i}]"
        ));
        cur = format!("vl{i}");
    }
    // A moving GIF or clip gets this many frames a second (each a new size along its track).
    let rate = 30;
    for (j, (_, a, b, moving, media)) in l.overlays.iter().enumerate() {
        let input = first_png + j;
        if let Some(mp) = media {
            g.push(edmedia::media_chain(mp, moving.as_ref(), input, &cur, j, (pw, ph), (*a, *b), rate));
            cur = format!("vo{j}");
            continue;
        }
        if let Some(mv) = moving {
            // The item's box, scaled about its middle and moved per frame (its input is the PNG looped, so a frame comes for every size).
            let (bx, by, bw, bh) = mv.bx;
            let (sx, sy, dx, dy) = (move_expr(&mv.pts, 2), move_expr(&mv.pts, 3), move_expr(&mv.pts, 0), move_expr(&mv.pts, 1));
            let (w0, h0) = (ex_num(pw as f64 * bw), ex_num(ph as f64 * bh));
            let (mx, my) = (ex_num(bx + bw / 2.0), ex_num(by + bh / 2.0));
            // A looped picture ends with the result (`shortest`); a single still one is held by `overlay` to the end
            // (with `shortest` the result would end after its one frame). S, 0.1.44: the same frames, -15 % time.
            let shortest = if mv.size_moves() { ":shortest=1" } else { "" };
            g.push(format!(
                "[{input}:v]format=rgba,scale=w='max(2,{w0}*({sx}))':h='max(2,{h0}*({sy}))':eval=frame[op{j}];[{cur}][op{j}]overlay=x='{pw}*({mx}+({dx}))-overlay_w/2':y='{ph}*({my}+({dy}))-overlay_h/2'{shortest}:enable='between(t,{a:.4},{b:.4})'[vo{j}]"
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
fn moving_blur_chain(mv: &Moving, cur: &str, i: usize, (pw, ph): (u32, u32), (a, b, st): (f64, f64, f64), mask: Option<usize>) -> Vec<String> {
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
    // A round / soft box: its fixed-size cut faded by the mask (edmedia.rs) before it is scaled to the moment's size.
    let faded = match mask {
        Some(mk) => format!("[kb{i}e]format=yuva420p[kb{i}q];[{mk}:v]format=gray,scale={fw}:{fh}[mk{i}];[kb{i}q][mk{i}]alphamerge[kb{i}f];[kb{i}f]"),
        None => format!("[kb{i}e]"),
    };
    vec![format!(
        "[{cur}]split=2[{cur}m][{cur}s];[{cur}s]gblur=sigma={sigma:.3}:steps=2:{on},split=2[kb{i}a][kb{i}b];[kb{i}a]crop={fw}:{fh}:0:0[kb{i}c];\
         [kb{i}b]scale=w='max({fw},{fw}/({wf}))':h='max({fh},{fh}/({hf}))':eval=frame[kb{i}d];\
         [kb{i}c][kb{i}d]overlay=x='-clip(({lx})*overlay_w,0,overlay_w-{fw})':y='-clip(({ty})*overlay_h,0,overlay_h-{fh})'[kb{i}e];\
         {faded}scale=w='max(2,{pw}*{wf})':h='max(2,{ph}*{hf})':eval=frame[bb{i}];\
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
/// `export` that `cancel_export { id }` can end (0.1.41): the command's way in.
pub fn export_cancellable(s: &Settings, src: &ClipMeta, opts: &ExportOpts, id: &str, progress: &mut dyn FnMut(f32)) -> Result<ClipMeta, String> {
    let guard = ExportGuard::new(id);
    with_cancel(&guard.flag, || export(s, src, opts, progress))
}

/// Run `f` with `flag` as this thread's export cancel flag (every ffmpeg the export starts reads it).
pub fn with_cancel<T>(flag: &std::sync::Arc<std::sync::atomic::AtomicBool>, f: impl FnOnce() -> T) -> T {
    let before = CANCEL.with(|c| c.replace(Some(flag.clone())));
    let out = f();
    CANCEL.with(|c| *c.borrow_mut() = before);
    out
}
pub fn export(s: &Settings, src: &ClipMeta, opts: &ExportOpts, progress: &mut dyn FnMut(f32)) -> Result<ClipMeta, String> {
    // 0.1.44: rows by role on one clip (its own entry of `track_map`, else the first; overrides): plain stream rows.
    let by_stream;
    let opts = if opts.track_map.is_empty() && opts.track_over.is_empty() {
        opts
    } else {
        let entry = opts.track_map.iter().find(|m| m.clip == src.id).or(opts.track_map.first());
        by_stream = rows_by_stream(opts, entry);
        &by_stream
    };
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
    let mut pieces = kept_ranges(opts, duration);
    // The finer of the library's and the file's rate: the snap never moves a cut by more than half a real frame.
    let nominal = src.fps.max(library::probe(&ffmpeg, &src_path, false).map(|p| p.fps).unwrap_or(0));
    if !NO_SNAP.with(|n| n.get()) {
        snap_to_frames(&ffmpeg, &src_path, &mut pieces, nominal, duration);
    }
    let pieces = pieces;
    let ranges: Vec<(f64, f64)> = pieces.iter().map(|k| (k.a, k.b)).collect();
    // 0.1.41: a range with its own speed lasts (b - a) / speed on the timeline.
    let speeds: Vec<f64> = pieces.iter().map(|k| k.speed).collect();
    let kept_src: f64 = ranges.iter().map(|(a, b)| b - a).sum();
    let kept: f64 = ranges.iter().zip(&speeds).map(|((a, b), sk)| (b - a) / sk).sum();
    let Some(&(_, last_end)) = ranges.last() else {
        return Err("The selection is too short".into());
    };
    if kept < 0.1 {
        return Err("The selection is too short".into());
    }
    // Gaps on the main video row (0.1.36): timeline seconds of black and silence before each range.
    let gaps: Vec<f64> = pieces.iter().map(|k| k.gap).collect();
    let gap_total: f64 = gaps.iter().sum();
    // The video's part of the result on the timeline: the kept ranges and the gaps between them.
    let span = kept + gap_total;
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
    // Black after the video (0.1.35), in output seconds; at most an hour.
    let tail = if opts.tail.is_finite() { opts.tail.clamp(0.0, 3600.0) } else { 0.0 };
    let tail = if tail < 0.001 { 0.0 } else { tail };
    // The video's own part of the result, then the whole result with the black after it.
    let out_video = span / speed;
    // Overlays, blur boxes and fades (on the result's own timeline, `out_total` long).
    let out_total = out_video + tail;
    let layers = layer_plan(opts, out_total)?;
    if k_count == 1 && same(speed, 1.0) && plain && opts.tracks.is_empty() && opts.audio.is_empty() && layers.is_none() && opts.canvas.is_none() && tail == 0.0 && gap_total == 0.0 && speeds.iter().all(|x| same(*x, 1.0)) && kept_src >= crate::clip::MIN_TRIM {
        util::log(&format!("export: lossless cut of {}: {base:.3}..{:.3} s", src.title, if to_end { duration } else { last_end }));
        let meta = crate::clip::trim(s, src, base, if to_end { duration } else { last_end }, true)?;
        util::log(&format!("export: lossless cut done, {:.1} MB", meta.size as f64 / 1e6));
        progress(100.0);
        return Ok(meta);
    }

    let titles = audio_titles(&ffmpeg, &src_path)?;
    let mut tempo = atempo_chain(speed);
    // Every file the mix needs is checked here, before the long encode starts.
    // Files that play through cuts may go on into the black after the video (kept seconds, before the speed).
    let mut plan = audio_plan(&ffmpeg, &src_path, opts, &titles, duration, base, total, k_count, speed, span + tail * speed)?;
    if (tail > 0.0 || gap_total > 0.0 || speeds.iter().any(|x| !same(*x, 1.0))) && matches!(plan, AudioPlan::Copy) {
        plan = AudioPlan::Tracks; // a copied track cannot be padded with silence, nor hold a gap
    }
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
    if tail > 0.0 {
        // Every audio result is as long as the picture with its black: silence after the clip's sound.
        tempo += &format!(",apad=whole_dur={out_total:.6}");
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
    // Where each gap lies on the result's timeline before the speed (start, length): black frames below.
    let mut gap_at: Vec<(f64, f64)> = Vec::new();
    // Each range: (seek, a, b, open end, look, placed, own speed).
    let mut cuts: Vec<(f64, f64, f64, bool, String, f64, f64)> = Vec::new();
    for (k, &(a, b)) in ranges.iter().enumerate() {
        if pieces[k].gap > 0.0 {
            gap_at.push((placed, pieces[k].gap));
            placed += pieces[k].gap;
        }
        // Half a millisecond early so rounding never drops the frame itself;
        // frames are at least 4 ms apart even at 240 fps.
        let seek = on_screen[k].map(|f| (f - 0.0005).max(0.0)).filter(|f| *f <= a).unwrap_or(a);
        // Only the last range in the result may read on to the file's end:
        // anything past an earlier range's end would land in the next one.
        let open_end = k + 1 == k_count && reaches_end(b);
        let look = if uniform {
            String::new()
        } else if pieces[k].blank {
            format!(",format=yuv420p,lutyuv=y={black}:u=128:v=128,{tags}")
        } else {
            let c = color_filter(&pieces[k].color, matrix, range).map(|f| format!(",{f}")).unwrap_or_default();
            format!("{c},format=yuv420p,{tags}")
        };
        cuts.push((seek, a, b, open_end, look, placed, speeds[k]));
        placed += (b - a) / speeds[k];
    }
    // 0.1.42: one input per range made the command too long for Windows (32767 characters) at about 100
    // ranges, and the owner's projects may now hold 500 clips. Neighbouring ranges in source order are read
    // from ONE input when the line would not fit otherwise: its frames go through one `select` (each range's
    // frames, from the frame on screen at its start to just before its end) and one `setpts` that places
    // each range as its own input did, so every frame lands where it did before. The gap between two ranges
    // read this way is decoded and dropped, so the allowed gap grows only as far as the line needs: an edit
    // that fits stays one input per range, exactly as before.
    let joinable = |k: usize, limit: f64| -> bool {
        let (p, c) = (&cuts[k - 1], &cuts[k]);
        // Same look, later in the source, and its on-screen frame not one of the previous range's own frames
        // (that range keeps the frames before its end - 0.5 ms, as its `trim` does).
        !p.3 && c.4 == p.4 && on_screen[k].is_some() && c.1 >= p.2 - 1e-6 && c.0 + 0.0005 >= p.2 - 0.0005 && c.0 - p.2 <= limit
    };
    let per_input = 64 + src_path.as_os_str().len();
    let others = 6000 + opts.audio.iter().map(|e| e.path.len() + 48).sum::<usize>() + (opts.overlays.len() + opts.blurs.len()) * 160;
    let budget = GROUP_BUDGET.with(|b| b.get()).unwrap_or(32_000).saturating_sub(others);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for limit in [-1.0, 2.0, 10.0, 60.0, f64::INFINITY] {
        groups.clear();
        for k in 0..cuts.len() {
            match groups.last_mut() {
                // ffmpeg refused a `select` of 150 ranges ("Cannot allocate memory"); 32 work.
                Some(g) if k > 0 && g.len() < MAX_SHARED && joinable(k, limit) => g.push(k),
                _ => groups.push(vec![k]),
            }
        }
        // Every input is a decoder open for the whole export: 150 of them ran ffmpeg out of memory while the graph
        // was set up ("Cannot allocate memory", a 30 fps 640x360 clip), so their number is held down as well.
        if groups.len() * per_input <= budget && groups.len() <= MAX_RANGE_INPUTS {
            break;
        }
    }
    if groups.len() < k_count {
        util::log(&format!("export: {k_count} ranges read through {} inputs (neighbours share one)", groups.len()));
    }
    let n_in = groups.len();
    let shared = n_in < k_count;
    // 0.1.44: where `interleave` joins the picture (several inputs, gaps, black after), it loses the last frame's
    // duration; the end is then held by a repeat of the last picture (`end_cut`). One input alone keeps it.
    let needs_end = n_in > 1 || tail > 0.0 || gap_total > 0.0;
    LAST_INPUTS.with(|n| n.set(n_in));
    for (g, members) in groups.iter().enumerate() {
        let first = &cuts[members[0]];
        let last = &cuts[*members.last().unwrap_or(&members[0])];
        let seek = first.0;
        // `-t` only bounds the reading; the exact end is the `trim`, because an
        // input's `-t` counts from its first frame, not from the seek point.
        video_inputs.push((seek, (!last.3).then(|| last.2 - seek + 0.5)));
        let label = if n_in == 1 { "vj".to_string() } else { format!("v{g}") };
        let look = &first.4;
        // A range with its own speed (0.1.41): its frames' times inside it are divided by it.
        let rel = |c: &(f64, f64, f64, bool, String, f64, f64)| if same(c.6, 1.0) { format!("max(0,T-{:.6})", c.1 - seek) } else { format!("max(0,T-{:.6})/{:.6}", c.1 - seek, c.6) };
        if members.len() == 1 {
            // The end is half a millisecond early too, the mirror of the seek: a
            // frame at (or within rounding of) `b` is left to the next range,
            // which takes it as the frame on screen at its start. Touching ranges
            // then share no frame and lose none, whatever the timestamp rounding.
            let trim = if first.3 { String::new() } else { format!("trim=end={:.6},", first.2 - seek - 0.0005) };
            let x = first.1 - seek;
            let one = if same(first.6, 1.0) { format!("if(eq(N,0),0,max(0,T-{x:.6}))") } else { format!("if(eq(N,0),0,max(0,T-{x:.6}))/{:.6}", first.6) };
            graph.push(format!("[{g}:v:0]{HWDL}{trim}setpts='({:.6}+{one})/{speed:.6}/TB'{}{look}[{label}]", first.5, if g + 1 == groups.len() && tail == 0.0 && needs_end { END_PAD } else { "" }));
            continue;
        }
        // Several ranges from one input: range j keeps [its on-screen frame, its end - 0.5 ms).
        let (mut sel, mut pts) = (Vec::new(), Vec::new());
        for (j, &k) in members.iter().enumerate() {
            let c = &cuts[k];
            let lo = (j > 0).then(|| c.0 - seek);
            let hi = (!c.3).then(|| c.2 - seek - 0.0005);
            let cond = |v: &str| match (lo, hi) {
                (Some(l), Some(h)) => format!("gte({v},{l:.6})*lt({v},{h:.6})"),
                (None, Some(h)) => format!("lt({v},{h:.6})"),
                (Some(l), None) => format!("gte({v},{l:.6})"),
                (None, None) => "1".to_string(),
            };
            sel.push(cond("t"));
            pts.push(format!("{}*({:.6}+{})", cond("T"), c.5, rel(c)));
        }
        graph.push(format!("[{g}:v:0]{HWDL}select='{}',setpts='if(eq(N,0),{:.6},{})/{speed:.6}/TB'{}{look}[{label}]", sel.join("+"), first.5, pts.join("+"), if g + 1 == groups.len() && tail == 0.0 && needs_end { END_PAD } else { "" }));
    }
    if n_in > 1 {
        let labels: String = (0..n_in).map(|k| format!("[v{k}]")).collect();
        graph.push(format!("{labels}interleave=nb_inputs={n_in}[vj]"));
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
    // The frame rate the export's own frames use (the speed-up cap, black in gaps and after the video): the
    // footage's real average rate, not a library number that can be a joined file's guess (0.1.41, R3-5: a
    // draft with one odd clip carried 203 fps into every black frame). The library's rate when it is plausible.
    let probed = library::probe(&ffmpeg, &src_path, false).map(|p| p.fps).unwrap_or(0);
    let cap = match (src.fps, probed) {
        (l, p) if l > 0 && (p == 0 || (l as f64) <= p as f64 * 1.5 + 1.0) => l,
        (_, p) if p > 0 => p,
        _ => 60,
    }
    .clamp(1, 240);
    // 0.1.44: the last frame lasts its whole frame. `interleave` drops the last frame's duration, so the file came
    // out a frame short (two at 0.5x; a player's length and the end of a join). The last range repeats its last
    // picture past its end (`tpad`, below) and the result is cut half a frame after its end: a repeat of the last
    // picture stands exactly at the end, takes the lost duration, and is never seen.
    let end_cut = |t: f64| format!("trim=end={:.6}", t + 0.5 / cap as f64);
    let mut video = if tail > 0.0 || !needs_end { "[vj]null".to_string() } else { format!("[vj]{}", end_cut(out_video)) };
    let fastest = speed * speeds.iter().copied().fold(1.0, f64::max);
    let any_speed = !same(speed, 1.0) || speeds.iter().any(|x| !same(*x, 1.0));
    if fastest > 1.0 + 1e-3 {
        // 0.1.44: 0.2 ms of slack: a frame due exactly on the target time came out a hair before it (float sums)
        // and was dropped, a frame missing at a cut and the next one half a frame late (the frame-exact checks).
        video += &format!(",select='if(gte(t,ld(0)-0.0002),st(0,max(ld(0),t-1/{cap})+1/{cap})*0+1,0)'");
    }
    if let Some(f) = color_filter(&pieces[0].color, matrix, range).filter(|_| uniform) {
        video += ",";
        video += &f;
    }
    // The picture's size: the source's, or the canvas's when there is one.
    let src_size = if src.width > 0 && src.height > 0 {
        Some((src.width, src.height))
    } else if layers.is_some() || opts.canvas.is_some() || tail > 0.0 || gap_total > 0.0 {
        Some(library::probe(&ffmpeg, &src_path, false).map(|p| (p.width, p.height)).filter(|(w, h)| *w > 0 && *h > 0).ok_or("Could not read the clip's picture size")?)
    } else {
        None
    };
    if let (true, Some((w, h))) = (tail > 0.0 && gap_total == 0.0, src_size) {
        // After the video (0.1.35): black frames at the clip's rate from the end of the video on,
        // laid after it by timestamp (as the kept ranges are joined), before the canvas and the
        // overlays, so those go on over the black.
        video += &format!(",format=yuv420p,{tags}[vk]");
        graph.push(std::mem::take(&mut video));
        graph.push(format!("color=c=black:s={w}x{h}:r={cap}:d={:.6},format=yuv420p,lutyuv=y={black}:u=128:v=128,setsar=1,{tags},settb=AVTB,setpts=PTS+{out_video:.6}/TB[vt]", tail + 1.0 / cap as f64));
        video = format!("[vk][vt]interleave=nb_inputs=2,{}", end_cut(out_total));
    } else if let (true, Some((w, h))) = (gap_total > 0.0, src_size) {
        // Gaps (0.1.36): black frames of each gap's length at the clip's rate, placed where the gap
        // is (output seconds), and the tail's after the video, all laid in by timestamp like the tail.
        let mut blacks: Vec<(f64, f64)> = gap_at.iter().map(|&(at, len)| (at / speed, len / speed)).collect();
        if tail > 0.0 {
            // A frame more than the black: the repeat at the end (`end_cut`).
            blacks.push((out_video, tail + 1.0 / cap as f64));
        }
        video += &format!(",format=yuv420p,{tags}[vk]");
        graph.push(std::mem::take(&mut video));
        let mut labels = "[vk]".to_string();
        for (i, (at, len)) in blacks.iter().enumerate() {
            graph.push(format!("color=c=black:s={w}x{h}:r={cap}:d={len:.6},format=yuv420p,lutyuv=y={black}:u=128:v=128,setsar=1,{tags},settb=AVTB,setpts=PTS+{at:.6}/TB[vg{i}]"));
            labels += &format!("[vg{i}]");
        }
        video = format!("{labels}interleave=nb_inputs={},{}", blacks.len() + 1, end_cut(out_total));
    }
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
    let clip_audio = video_inputs.len();
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
            // A stream copy loses the MP4 track titles (R3-4): the editor and joins tell the tracks apart by them.
            for (i, t) in titles.iter().enumerate() {
                if let Some(t) = t.as_ref().filter(|t| !t.is_empty()) {
                    audio_args.extend([format!("-metadata:s:a:{i}"), format!("title={t}")]);
                }
            }
        }
        AudioPlan::Tracks => {
            let track_ch = if gap_total > 0.0 { probe_audio(&ffmpeg, &src_path).map(|p| p.channels).unwrap_or_default() } else { Vec::new() };
            for (i, t) in titles.iter().enumerate() {
                // A gap is silence in the track's own layout (mono stays mono).
                let layout = if track_ch.get(i).copied().unwrap_or(2) == 1 { "mono" } else { "stereo" };
                graph.push(cut_chain_gaps(&format!("[{clip_audio}:a:{i}]"), &local, &gaps, &speeds, &format!("t{i}"), &tempo, &format!("[a{i}]"), (MIX_RATE, layout)));
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
            // 0.1.44: many audio clips (a montage's ungrouped pieces each name their source's file) made one input
            // each, every one with its full path on the command line, which Windows cuts at 32767 characters. Past
            // SHARE_FILES_FROM of them each FILE is one input, read from its start, and every clip of it takes its
            // stretch with `atrim` (sample-exact; the decoded sound is small beside the picture's).
            let n_files = rows.iter().filter(|r| matches!(r.source, Source::File { .. })).count();
            let share = n_files > SHARE_FILES_FROM;
            let mut shared: Vec<(String, usize)> = Vec::new();
            let mut taps: Vec<((usize, u32), usize, usize)> = Vec::new(); // ((input, stream), uses, next)
            if share {
                for row in rows.iter() {
                    if let Source::File { path, stream, .. } = &row.source {
                        let input = match shared.iter().find(|(p, _)| p == path) {
                            Some((_, i)) => *i,
                            None => {
                                let i = first_file + file_inputs.len();
                                file_inputs.push((path.clone(), 0.0, -1.0));
                                shared.push((path.clone(), i));
                                i
                            }
                        };
                        match taps.iter_mut().find(|t| t.0 == (input, *stream)) {
                            Some(t) => t.1 += 1,
                            None => taps.push(((input, *stream), 1, 0)),
                        }
                    }
                }
                for ((input, stream), uses, _) in &taps {
                    let outs: String = (0..*uses).map(|u| format!("[fs{input}_{stream}_{u}]")).collect();
                    graph.push(format!("[{input}:a:{stream}]asplit={uses}{outs}"));
                }
            }
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
                    Source::File { path, seek, len, at, channels, stream } if share => {
                        let input = shared.iter().find(|(p, _)| p == path).map(|x| x.1).unwrap_or(first_file);
                        let tap = taps.iter_mut().find(|t| t.0 == (input, *stream)).map(|t| { t.2 += 1; t.2 - 1 }).unwrap_or(0);
                        let samples = (at * MIX_RATE as f64).round() as u64;
                        let delay = if samples > 0 { format!(",adelay=delays={samples}S:all=1") } else { String::new() };
                        format!("[fs{input}_{stream}_{tap}]asetpts=PTS-STARTPTS,atrim=start={seek:.6}:end={:.6},asetpts=PTS-STARTPTS,{}{delay},{gain}[r{j}]", seek + len, to_stereo(*channels))
                    }
                    Source::File { path, seek, len, at, channels, stream } => {
                        let input = first_file + file_inputs.len();
                        file_inputs.push((path.clone(), *seek, *len));
                        let samples = (at * MIX_RATE as f64).round() as u64;
                        let delay = if samples > 0 { format!(",adelay=delays={samples}S:all=1") } else { String::new() };
                        format!("[{input}:a:{stream}]{},asetpts=PTS-STARTPTS{delay},{gain}[r{j}]", to_stereo(*channels))
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
                graph.push(cut_chain_gaps("[mix]", &local, &gaps, &speeds, "m", &tempo, "[a]", (MIX_RATE, "stereo")));
            } else {
                // The cut mix first, then the files that play through the cuts on top of it, on the
                // result's own timeline. `duration=first`: the result stays as long as the video.
                // With black after the video the cut mix is padded first, so the files go on into it.
                let pad = if tail > 0.0 { format!("apad=whole_dur={:.6}", span + tail * speed) } else { "anull".to_string() };
                graph.push(cut_chain_gaps("[mix]", &local, &gaps, &speeds, "m", &pad, "[mc]", (MIX_RATE, "stereo")));
                graph.push(format!("[mc]{thru}amix=inputs={}:duration=first:normalize=0,{tempo}[a]", n_thru + 1));
            }
            // 0.1.45: a mix that can pass full scale (a level over 100 % anywhere, or more than one source summed) ends
            // in a limiter, so 200 % is louder but never clipped (the owner's crackle). Sound under the ceiling passes
            // as it is; a mix that cannot pass full scale keeps its command exactly.
            let loud = rows.len() > 1 || rows.iter().any(|r| r.volume > 1.0 + 1e-6 || r.regions.iter().any(|g| r.volume * g.2 > 1.0 + 1e-6));
            if loud {
                if let Some(last) = graph.last_mut().filter(|g| g.ends_with("[a]")) {
                    last.truncate(last.len() - 3);
                    last.push_str("[al]");
                    graph.push(format!("[al]{LIMITER}[a]"));
                }
            }
            maps.extend(["-map", "[a]"].map(String::from));
            audio_args.extend(["-metadata:s:a:0".to_string(), format!("title={title}"), "-disposition:a:0".into(), "default".into()]);
            aac(&mut audio_args);
        }
    }

    // The overlay pictures are the last inputs.
    let first_png = first_file + file_inputs.len();
    let loop_rate = cap.min(60);
    // Their inputs (PNGs, pictures / GIFs / clips, then the round blur boxes' masks: edmedia.rs).
    let mut _masks = OverlayFiles(Vec::new());
    let mut layer_args: Vec<String> = Vec::new();
    if let Some(l) = &layers {
        // On a canvas, overlay PNGs arrive at the canvas's size and blur boxes are fractions of it.
        let size = canvas.as_ref().map(|c| (c.width, c.height)).or(src_size).ok_or("Could not read the clip's picture size")?;
        let (files, masks) = edmedia::write_blur_masks(l, size)?;
        _masks = files;
        graph.extend(layer_chain(l, first_png, size, out_total, &masks));
        layer_args = edmedia::layer_input_args(l, &masks, loop_rate, out_total);
    }
    // Keyframe tracks can make the graph long; Windows cuts a command line at 32767 characters, so a
    // long graph goes to a file (`-/filter_complex`, ffmpeg 7+). Short ones stay on the line as before.
    let graph_joined = graph.join(";");
    let mut _graph_file = OverlayFiles(Vec::new());
    // 0.1.46: the graph twice, for decoding on the CPU (as before) and on the graphics card (each range input's
    // frames first brought back to memory: `hwdownload`, as NV12, the same pixels as the CPU's yuv420p).
    let mut graph_args: Vec<[String; 2]> = Vec::new();
    for dl in ["", "hwdownload,format=nv12,"] {
        let graph_text = graph_joined.replace(HWDL, dl);
        graph_args.push(if graph_text.len() > 24_000 {
            let dir = overlay_dir();
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}.txt", util::new_id()));
            fs::write(&path, &graph_text).map_err(|e| e.to_string())?;
            _graph_file.0.push(path.clone());
            ["-/filter_complex".into(), path.to_string_lossy().into_owned()]
        } else {
            ["-filter_complex".into(), graph_text]
        });
    }

    let title = format!("{} (edit)", src.title);
    // Beside the source; from a draft, into its folder in the clips folder.
    let (dir, folder) = crate::clip::output_dir(s, src);
    let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
    let src_s = src_path.to_string_lossy().into_owned();

    let build_with = |enc: &str, hw: bool| -> Vec<String> {
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-nostats", "-y"].map(String::from).to_vec();
        for (seek, len) in &video_inputs {
            if hw {
                // 0.1.46 (S: CPU -44 to -68 %): the range's decoding on the graphics card.
                a.extend(["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"].map(String::from));
                if HW_TEST.with(|t| t.get()) == 1 {
                    a.extend(["-hwaccel_device", "9"].map(String::from)); // test: a card that is not there
                }
            }
            if video_inputs.len() > 4 {
                // Every range has its own decoder, all open at once; fewer
                // threads each keeps memory in check on a 1440p clip.
                a.extend(["-threads", "4"].map(String::from));
            }
            // 0.1.41, the hang: a joined file whose picture changes its colour tags part way (clips of other
            // settings) made ffmpeg rebuild the whole graph there, and a rebuild with inputs that had already
            // ended never started again. Each range input keeps the graph it was set up with.
            a.extend(["-reinit_filter", "0"].map(String::from));
            a.extend(["-ss".to_string(), format!("{seek:.6}")]);
            if let Some(l) = len {
                a.extend(["-t".to_string(), format!("{l:.6}")]);
            }
            a.extend(["-i".to_string(), src_s.clone()]);
        }
        if needs_clip {
            a.extend(["-reinit_filter", "0"].map(String::from));
            a.extend(["-ss".to_string(), format!("{base:.6}")]);
            if !to_end {
                a.extend(["-t".to_string(), format!("{total:.6}")]);
            }
            a.extend(["-i".to_string(), src_s.clone()]);
        }
        for (path, seek, len) in &file_inputs {
            if *len < 0.0 {
                // One input for every clip of this file (0.1.44): read from its start, each clip cut in the graph.
                a.extend(["-i".to_string(), path.clone()]);
                continue;
            }
            a.extend(["-ss".to_string(), format!("{seek:.6}"), "-t".into(), format!("{len:.6}"), "-i".into(), path.clone()]);
        }
        // The overlay pictures, media items and masks (edmedia::layer_input_args).
        a.extend(layer_args.iter().cloned());
        a.extend(graph_args[hw as usize].iter().cloned());
        a.extend(maps.iter().cloned());
        a.extend(video_codec_args(enc));
        if speeds.iter().any(|x| !same(*x, 1.0)) {
            // 0.1.44: pieces at different speeds space their frames differently, and B-frames then made the MP4 end
            // early: its frame-order delay at the end (two frames of 0.5x) was longer than at the start, so the file
            // said it ended up to a few frames before its last frame. Without B-frames there is no delay.
            a.extend(["-bf", "0"].map(String::from));
        }
        // Frames keep their own timestamps (no CFR duplication); vfr only drops exact repeats.
        a.extend(["-fps_mode", "vfr"].map(String::from));
        if any_speed || shared {
            // Ranges sharing an input (0.1.42) arrive with the source's frame rate on them, so the encoder took
            // 1/fps as its clock and moved every frame onto that grid, up to a frame and a half late at the cuts.
            // A changed speed puts frames between the source frame rate's ticks: without a fine clock the encoder
            // rounded them onto that rate's grid, up to a frame late against the sound (0.1.41, the sync checks).
            a.extend(["-enc_time_base:v", "1/90000"].map(String::from));
        }
        // A keyframe each second of output, like the recorder, so the result can be trimmed losslessly too. 0.1.44:
        // and the repeat at the very end (`end_cut`), so it is written last (a frame before it that waits for it,
        // with B-frames, was written after it, and its lost duration was then the file's end).
        if needs_end {
            a.extend(["-force_key_frames".to_string(), format!("expr:gte(t,n_forced*1)+gte(t,{:.6})", out_total - 0.25 / cap as f64)]);
        } else {
            a.extend(["-force_key_frames", "expr:gte(t,n_forced*1)"].map(String::from));
        }
        a.extend(audio_args.iter().cloned());
        if layers.is_some() {
            // A moving picture's input loops for a second more than the result: without a bound the video ran on
            // past the sound by that much (0.1.41: the owner's montage came out 166.2 s against 165.2 s of sound).
            // Half a frame more: the repeat at the end (`end_cut`) is kept.
            a.extend(["-t".to_string(), format!("{:.6}", out_total + if needs_end { 0.5 / cap as f64 } else { 0.0 })]);
        }
        a.extend(["-metadata".to_string(), format!("title={title}"), "-movflags".into(), "+faststart".into(), "-progress".into(), "pipe:1".into()]);
        a.push(out.to_string_lossy().into_owned());
        a
    };

    let build = |enc: &str| build_with(enc, false);
    let expected = out_total;
    if DRY_RUN.with(|d| d.borrow().is_some()) {
        // `--editor-test`: the command (libx264, paths as placeholders) instead of an encode.
        let data = util::data_dir().to_string_lossy().into_owned();
        let tmp = util::tmp_dir().to_string_lossy().into_owned();
        let out_s = out.to_string_lossy().into_owned();
        let _ = fs::remove_file(&out);
        let args: Vec<String> = build("x264").into_iter().map(|a| a.replace(&out_s, "<out>").replace(&src_s, "<src>").replace(&tmp, "<tmp>").replace(&data, "<data>")).collect();
        DRY_RUN.with(|d| *d.borrow_mut() = Some(args));
        return Err("dry run".into());
    }
    let enc = pick_encoder(&ffmpeg, &s.encoder);
    // Windows refuses a command line over 32767 characters (R3-2): say so plainly instead of "os error 206".
    let line_len: usize = build(&enc).iter().map(|a| a.len() + 3).sum::<usize>() + ffmpeg.as_os_str().len() + 3;
    if line_len > 32_000 {
        // 0.1.44: pieces share inputs and so do the clips of one audio file; what is left to grow the line is the
        // number of DIFFERENT audio files. Say which it is.
        let files_len: usize = file_inputs.iter().map(|f| f.0.len() + 40).sum();
        util::log(&format!("export: the command would be {line_len} characters ({k_count} pieces, {} audio file inputs, {files_len} characters of them): refused", file_inputs.len()));
        if files_len * 2 > line_len {
            return Err(format!("This edit uses too many different audio files ({}) to export in one go. Export it in two parts, or use fewer separate audio files.", file_inputs.len()));
        }
        return Err(format!("This edit has too many pieces ({k_count}) to export in one go. Export it in two parts, or join some pieces first."));
    }
    let n_layers = layers.as_ref().map(|l| l.overlays.len() + l.blurs.len()).unwrap_or(0);
    let plan_name = match &plan { AudioPlan::Silent => "no sound".to_string(), AudioPlan::Copy => "tracks copied".into(), AudioPlan::Tracks => format!("{} tracks", titles.len()), AudioPlan::Mix { rows, .. } => format!("a mix of {} rows", rows.len()) };
    util::log(&format!("export: re-encode of {}: {k_count} ranges, {:.1} s out, {plan_name}, {n_layers} layers, {}{}, encoder {enc}", src.title, out_total, if gap_total > 0.0 { "gaps, " } else { "" }, if tail > 0.0 { "a tail" } else { "no tail" }));
    let started = std::time::Instant::now();
    // The bar never goes back (a retry on the CPU starts its own count from 0: R3-11).
    let top = std::cell::Cell::new(0f32);
    let mut forward = |p: f32| {
        if p > top.get() {
            top.set(p);
            progress(p);
        }
    };
    // 0.1.46: the pieces decoded on the graphics card when it can (NVIDIA: the encoder is NVENC; the setting not
    // "off"; H.264 / HEVC 8-bit 4:2:0; at most HW_MAX_INPUTS decoders), else on the CPU as before. Any failure of
    // that run (at its start, part way, a card without the decoder) is retried at once on the CPU: one log line.
    let hw = match hw_decode_plan(&ffmpeg, s, &enc, &src_path, video_inputs.len()) {
        Ok(()) => true,
        Err(why) => {
            if s.hw_decode != "off" && enc == "nvenc" {
                util::log(&format!("export: decoding on the CPU ({why})"));
            }
            false
        }
    };
    // 0.1.46, smart rendering: a cuts-only export of one clip whose packets allow it re-encodes only each range's
    // head and copies the rest (`smartcut`). Any refusal or failure: the full re-encode below, one log line.
    let cuts_only = k_count >= 2 && same(speed, 1.0) && speeds.iter().all(|x| same(*x, 1.0)) && plain && opts.tracks.is_empty() && opts.audio.is_empty() && layers.is_none() && opts.canvas.is_none() && tail == 0.0 && gap_total == 0.0;
    let mut smart_done = false;
    if cuts_only && (SMART_READY || SMART_TRY.with(|x| x.get())) && s.smart_cuts != "off" && !SMART_OFF.with(|x| x.get()) {
        match smartcut::eligible(&ffmpeg, &src_path, &enc) {
            Ok(()) => {
                let t0 = std::time::Instant::now();
                match smartcut::export(&ffmpeg, &src_path, &ranges, &titles, &title, &out, &mut forward) {
                    Ok((heads, copied)) => {
                        util::log(&format!("export: smart render, {heads} heads encoded, {copied} frames copied, in {:.1} s", t0.elapsed().as_secs_f64()));
                        smart_done = true;
                    }
                    Err(e) if e == CANCELLED => return Err(e),
                    Err(e) => {
                        let _ = fs::remove_file(&out);
                        util::log(&format!("export: smart render failed ({e}), re-encoding all"));
                    }
                }
            }
            Err(why) => util::log(&format!("export: no smart render ({why})")),
        }
    }
    SMART_USED.with(|x| x.set(smart_done));
    LAST_HW.with(|l| l.set(false));
    let mut result = if smart_done {
        Ok(())
    } else if hw {
        HW_RUN.with(|h| h.set(true));
        let r0 = run_with_progress(&ffmpeg, &build_with(&enc, true), expected, &mut forward);
        HW_RUN.with(|h| h.set(false));
        LAST_HW.with(|l| l.set(r0.is_ok()));
        match r0 {
            Err(e) if e != CANCELLED => {
                util::log(&format!("export: decoding on the graphics card failed ({}), decoding on the CPU", e.lines().next().unwrap_or("")));
                let _ = fs::remove_file(&out);
                let base_pct = top.get();
                let mut again = |p: f32| forward(base_pct + (99.0 - base_pct) * p / 100.0);
                run_with_progress(&ffmpeg, &build(&enc), expected, &mut again)
            }
            r => r,
        }
    } else {
        run_with_progress(&ffmpeg, &build(&enc), expected, &mut forward)
    };
    if let Err(e) = &result {
        if enc != "x264" && e != CANCELLED && !e.starts_with("The export stopped") {
            // A hardware encoder can be busy (the recorder and a game may hold its sessions); the CPU always works.
            util::log(&format!("export: {enc} failed ({e}), retrying with libx264"));
            let _ = fs::remove_file(&out);
            let base_pct = top.get();
            let mut again = |p: f32| forward(base_pct + (99.0 - base_pct) * p / 100.0);
            result = run_with_progress(&ffmpeg, &build("x264"), expected, &mut again);
        }
    }
    if let Err(e) = result {
        let _ = fs::remove_file(&out);
        if e == CANCELLED {
            util::log(&format!("export: cancelled after {:.1} s", started.elapsed().as_secs_f64()));
            return Err(e);
        }
        // The full command only when it failed, without the user's folder names where that is easy.
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let line = build(&enc).join(" ");
        let line = if home.len() > 3 { line.replace(&home, "%USERPROFILE%") } else { line };
        util::log(&format!("export failed after {:.1} s: {e}\n  command: {line}", started.elapsed().as_secs_f64()));
        if e.starts_with("The export stopped") {
            return Err(e);
        }
        return Err(format!("Export failed: {e}"));
    }
    util::log(&format!("export: done in {:.1} s, {:.1} MB", started.elapsed().as_secs_f64(), fs::metadata(&out).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0)));
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
    if std::env::var("GLINT_EDITOR_ONLY").is_ok_and(|v| v == "smart") {
        smart_checks(r, &ffmpeg, &s, &dir);
        return;
    }
    if std::env::var("GLINT_EDITOR_ONLY").is_ok_and(|v| v == "hwbench") {
        hw_bench(r, &ffmpeg, &s, &dir);
        return;
    }
    if std::env::var("GLINT_EDITOR_ONLY").is_ok_and(|v| v == "hw") {
        hw_checks(r, &ffmpeg, &s, &dir);
        return;
    }
    if std::env::var("GLINT_EDITOR_ONLY").is_ok_and(|v| v == "looks") {
        // A quick run of only the limiter and look checks (0.1.45), while working on them.
        let clip = dir.join("lk.mp4");
        if flash_clip(&ffmpeg, &clip, 30, 8.0, false, 0.0) {
            let src = ClipMeta { fps: 30, ..library::register(&ffmpeg, &clip, "Lk", "Test Game", "clip") };
            limiter_checks(r, &ffmpeg, &s, &src, &dir);
        }
        look_tg_checks(r, &ffmpeg, &s, &dir);
        return;
    }
    if std::env::var("GLINT_EDITOR_ONLY").is_ok_and(|v| v == "groups") {
        // A quick run of only the shared-input checks (0.1.42), while working on them.
        group_checks(r, &ffmpeg, &s, &dir);
        snap_checks(r, &ffmpeg, &s, &dir);
        return;
    }
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
    edmedia::media_checks(r, &ffmpeg, &s, &src, &src_file);
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
    let moving = |png: String, from: f64, to: f64, track: Vec<MovePoint>| Overlay { png, from, to, x: base.0, y: base.1, w: base.2, h: base.3, track, media: None };

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

    // --- 1b. 0.1.44 (S): an overlay that only moves is one still picture, not a looped one: frame for frame the
    // same result (decoded frames' checksums and times), over the clip's own picture and across a cut.
    {
        let track = vec![mv(0.3, 0.0, 0.0, 1.5, 1.5, false), mv(2.2, 0.5, 0.3, 1.5, 1.5, true)];
        let opts = ExportOpts { segments: vec![Range { start: 0.0, end: 2.0, ..Default::default() }, Range { start: 5.0, end: 7.0, ..Default::default() }], overlays: vec![moving(make_box("kf1b.png"), 0.2, 3.6, track)], ..Default::default() };
        let md5s = |p: &Path| -> Vec<String> {
            let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"]).arg(p).args(["-map", "0:v:0", "-f", "framemd5", "-"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output() else { return Vec::new() };
            String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('#')).map(|l| { let f: Vec<&str> = l.split(',').map(str::trim).collect(); format!("{} {}", f.get(2).unwrap_or(&""), f.last().unwrap_or(&"")) }).collect()
        };
        STILL_AS_LOOP.with(|c| c.set(true));
        let t0 = std::time::Instant::now();
        let looped = ex(&opts);
        // Each export deletes its overlay pictures when it ends: the second one gets its own.
        let opts = ExportOpts { overlays: vec![Overlay { png: make_box("kf1c.png"), ..opts.overlays[0].clone() }], ..opts };
        let t_loop = t0.elapsed().as_secs_f64();
        STILL_AS_LOOP.with(|c| c.set(false));
        let t1 = std::time::Instant::now();
        let still = ex(&opts);
        let t_still = t1.elapsed().as_secs_f64();
        let still_cmd = export_args(s, src, &opts).map(|a| a.join(" ")).unwrap_or_default();
        match (looped, still) {
            (Ok(a), Ok(b)) => {
                let (ma, mb) = (md5s(Path::new(&a.path)), md5s(Path::new(&b.path)));
                let differ = ma.iter().zip(&mb).filter(|(x, y)| x != y).count();
                r.check("an overlay that only moves: one still picture, frame for frame the same as looped", ma.len() > 100 && ma.len() == mb.len() && differ == 0 && !still_cmd.contains("-loop 1"), &format!("{} vs {} frames, {differ} differ; {t_loop:.1} s looped, {t_still:.1} s still", ma.len(), mb.len()));
                let _ = fs::remove_file(&a.path);
                let _ = fs::remove_file(&b.path);
            }
            (a, b) => r.check("an overlay that only moves: one still picture", false, &format!("{:?} / {:?}", a.err(), b.err())),
        }
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
    let blur = BlurBox { x: 0.05, y: 0.05, w: 0.25, h: 0.25, from: 0.0, to: 2.8, strength: 0.03, track: vec![mv(0.0, 0.0, 0.0, 1.0, 1.0, false), mv(2.0, 0.5, 0.45, 1.2, 1.2, false)], ..Default::default() };
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

/// `band_db_at` as a player hears it from the start: decoded from the file's first packet, no
/// seek (an input `-ss` may hide what the first packets hold), `from..to` cut by `atrim`.
fn band_db_head(ffmpeg: &Path, file: &Path, stream: u32, freq: u32, from: f64, to: f64) -> Option<f64> {
    let band = format!("bandpass=f={freq}:width_type=q:w=8");
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostats", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{stream}"), "-af", &format!("atrim=start={from:.3}:end={to:.3},{band},{band},volumedetect"), "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines().find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse::<f64>().ok()))
}

/// The first `n` packets of audio stream `stream`: (pts, duration) in seconds, and the stream's start time.
fn audio_head(ffmpeg: &Path, file: &Path, stream: u32, n: usize) -> (Vec<(f64, f64)>, f64) {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return (Vec::new(), f64::NAN) };
    let run = |args: &[&str]| -> String {
        util::hidden_cmd(&ffprobe).args(args).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let sel = format!("a:{stream}");
    let pk = run(&["-v", "error", "-select_streams", &sel, "-show_entries", "packet=pts_time,duration_time", "-of", "csv=p=0", "-read_intervals", "%+#40"]);
    let packets = pk.lines().take(n).filter_map(|l| { let mut it = l.split(','); Some((it.next()?.trim().parse().ok()?, it.next()?.trim().parse().unwrap_or(0.0))) }).collect();
    let st = run(&["-v", "error", "-select_streams", &sel, "-show_entries", "stream=start_time", "-of", "default=nw=1:nk=1"]).trim().parse().unwrap_or(f64::NAN);
    (packets, st)
}

/// 0.1.35, a user's report: "after exporting a clip with an imported audio file, the imported audio
/// does not play until I skip back". Every way a song can sit in an export: through cuts or cut
/// with the video, at the start or later, the clip's own sound kept, muted or deleted, with cuts.
/// For each: the audio stream starts at 0 with its first packets in order and without a gap, and
/// the song is heard where it belongs when the file is decoded from its start (as a player does).
#[allow(clippy::too_many_arguments)]
fn song_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path, f_mic: u32, keep: &Path) {
    let f_song = 700u32;
    let song = dir.join("Imported Song.mp3");
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={f_song}:sample_rate=44100:duration=12"), "-ac", "2", "-c:a", "libmp3lame", "-b:a", "192k", &song.to_string_lossy()]);
    r.check("song: an MP3 to import is made", made.is_ok(), made.as_ref().err().map(String::as_str).unwrap_or(""));
    if made.is_err() {
        return;
    }
    let ref_song = band_db_head(ffmpeg, &song, 0, f_song, 1.0, 5.0);
    let ref_mic = band_db_at(ffmpeg, Path::new(&src.path), 2, f_mic, 0.2, 7.8);
    let song_s = song.to_string_lossy().into_owned();
    let track = |index: u32, volume: f32| AudioEdit { source: "track".into(), index, volume, ..Default::default() };
    let file = |through: bool, offset: f64| AudioEdit { source: "file".into(), path: song_s.clone(), offset, volume: 1.0, through, ..Default::default() };
    let seg = |v: &[(f64, f64)]| v.iter().map(|&(start, end)| Range { start, end, ..Default::default() }).collect::<Vec<_>>();
    let _ = fs::create_dir_all(keep);
    // (label, rows, kept ranges, where the song starts in the result, the result's length, the mic heard)
    let cases: Vec<(&str, Vec<AudioEdit>, Vec<Range>, f64, f64, bool)> = vec![
        ("through row at the start, clip sound kept", vec![track(1, 1.0), track(2, 1.0), file(true, 0.0)], Vec::new(), 0.0, 8.0, true),
        ("old row at the start, clip sound kept", vec![track(1, 1.0), track(2, 1.0), file(false, 0.0)], Vec::new(), 0.0, 8.0, true),
        ("through row at 2 s, clip sound kept", vec![track(1, 1.0), track(2, 1.0), file(true, 2.0)], Vec::new(), 2.0, 8.0, true),
        ("old row at 2 s, clip sound kept", vec![track(1, 1.0), track(2, 1.0), file(false, 2.0)], Vec::new(), 2.0, 8.0, true),
        ("through row at the start, clip sound muted", vec![track(1, 0.0), track(2, 0.0), file(true, 0.0)], Vec::new(), 0.0, 8.0, false),
        ("through row at the start, clip sound deleted", vec![file(true, 0.0)], Vec::new(), 0.0, 8.0, false),
        ("old row at the start, clip sound deleted", vec![file(false, 0.0)], Vec::new(), 0.0, 8.0, false),
        ("through row at the start, cut 2..3", vec![track(2, 1.0), file(true, 0.0)], seg(&[(0.0, 2.0), (3.0, 8.0)]), 0.0, 7.0, true),
        ("through row at the start, the clip's start cut", vec![track(2, 1.0), file(true, 0.0)], seg(&[(1.5, 8.0)]), 0.0, 6.5, true),
        ("old row at the start, the clip's start cut", vec![track(2, 1.0), file(false, 0.0)], seg(&[(1.5, 8.0)]), 0.0, 6.5, true),
    ];
    for (k, (label, audio, segments, at, len, mic)) in cases.into_iter().enumerate() {
        let opts = ExportOpts { audio, segments, ..Default::default() };
        match export(s, src, &opts, &mut |_| {}) {
            Ok(m) => {
                let f = Path::new(&m.path);
                let _ = fs::copy(f, keep.join(format!("song{k}.mp4")));
                let (packets, start) = audio_head(ffmpeg, f, 0, 12);
                // In order, from 0, each packet where the last one ended.
                let ordered = packets.len() >= 10 && packets[0].0.abs() < 0.03 && packets.windows(2).all(|w| (w[1].0 - (w[0].0 + w[0].1)).abs() < 0.002);
                let p = library::probe(ffmpeg, f, true);
                let dur = p.as_ref().map(|p| p.duration).unwrap_or(0.0);
                let first = band_db_head(ffmpeg, f, 0, f_song, at + 0.05, at + 0.95);
                let before = if at > 0.5 { band_db_head(ffmpeg, f, 0, f_song, 0.1, at - 0.2) } else { None };
                let later = band_db_head(ffmpeg, f, 0, f_song, at + 2.0, at + 3.0);
                let mic_lv = band_db_head(ffmpeg, f, 0, f_mic, 0.1, 0.9);
                let heard = |x: Option<f64>| matches!((ref_song, x), (Some(r0), Some(v)) if near(v, r0, 1.5));
                let ok = start.abs() < 0.03
                    && ordered
                    && near(dur, len, 0.1)
                    && heard(first)
                    && heard(later)
                    && before.map_or(true, |b| ref_song.is_some_and(|r0| b < r0 - 25.0))
                    && (if mic { matches!((ref_mic, mic_lv), (Some(r0), Some(v)) if near(v, r0, 1.5)) } else { mic_lv.map_or(true, |v| ref_mic.is_some_and(|r0| v < r0 - 40.0)) });
                let pk: Vec<String> = packets.iter().take(4).map(|(a, b)| format!("{a:.4}+{b:.4}")).collect();
                r.check(
                    &format!("song {label}: heard from its first second on, as a player plays it from the start"),
                    ok,
                    &format!("start {start:.4}, packets {pk:?}, {dur:.2} s; song first second {first:?}, later {later:?}, before {before:?} (file {ref_song:?}); mic {mic_lv:?} ({ref_mic:?})"),
                );
            }
            Err(e) => r.check(&format!("song {label}"), false, &e),
        }
    }
}

/// The commands for options without gaps (hashes of `export_args`). Recorded with 0.1.35; 0.1.41 changed every one on
/// purpose (`-reinit_filter 0` on the range inputs: the hang) and they were recorded again; 0.1.44 changed every one on
/// purpose too (frame-exact cuts: the repeat that holds the end of a joined picture, `end_cut` / `END_PAD`, a key
/// frame on it, 0.2 ms of slack in the speed cap) and they were recorded again; 0.1.45 changed the three that mix more
/// than one source into one track on purpose (the limiter, `LIMITER`, at the end of such a mix): recorded again.
const COMMANDS_0135: [(&str, &str); 5] = [
    ("two ranges, the clip's tracks", "174f7bbc937ec0e7"),
    ("moved parts at 2x, a track and a through file", "0ee1bf3633e53e1e"),
    ("one range, a file cut with the video, fades, tail 1.5 s", "94b23f2d014d29f2"),
    ("whole clip, an overlay picture, tail 2 s", "44c129f68883afe7"),
    ("three ranges, a blank one and a colored one, levels", "14f808d9c7336776"),
];

/// The option sets of `COMMANDS_0135`, in that order.
fn commands_0135(tone: &str, png: &str) -> Vec<ExportOpts> {
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    let track = |index: u32, volume: f32| AudioEdit { source: "track".into(), index, volume, ..Default::default() };
    let file = |through: bool, offset: f64| AudioEdit { source: "file".into(), path: tone.into(), offset, trim_start: 1.0, length: 3.0, volume: 1.0, through, ..Default::default() };
    vec![
        ExportOpts { segments: vec![seg(0.5, 2.5), seg(5.5, 7.5)], ..Default::default() },
        ExportOpts { segments: vec![seg(5.5, 7.5), seg(0.5, 2.5)], ordered: true, speed: 2.0, audio: vec![track(2, 1.0), file(true, 1.0)], ..Default::default() },
        ExportOpts { segments: vec![seg(1.0, 6.0)], audio: vec![track(1, 0.5), file(false, 2.0)], fade_in: 0.5, fade_out: 1.0, tail: 1.5, ..Default::default() },
        ExportOpts { overlays: vec![Overlay { png: png.into(), from: 1.0, to: 9.0, ..Default::default() }], tail: 2.0, ..Default::default() },
        ExportOpts {
            segments: vec![seg(0.0, 2.0), Range { start: 2.0, end: 3.0, blank: true, ..Default::default() }, Range { start: 4.0, end: 6.0, color: Some(Color { brightness: 1.3, ..Default::default() }), ..Default::default() }],
            audio: vec![AudioEdit { regions: vec![GainRegion { start: 1.0, end: 2.5, gain: 0.0 }], ..track(1, 1.0) }, track(2, 1.5)],
            ..Default::default()
        },
    ]
}

/// Options without a gap give the command 0.1.35 gave, argument for argument.
fn command_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, tone: &str) {
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    let png = odir.join("cmd-ov.png");
    for (k, opts) in commands_0135(tone, &png.to_string_lossy()).into_iter().enumerate() {
        // The export removes its overlay PNGs: one made for each.
        let _ = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=white@0.5:s=64x36,format=rgba", "-frames:v", "1", &png.to_string_lossy()]);
        let (label, want) = COMMANDS_0135[k];
        match export_args(s, src, &opts) {
            Ok(args) => {
                let got = args_hash(&args);
                r.line(&format!("INFO command {label}: {got}"));
                if want.is_empty() {
                    r.line(&format!("INFO   {}", args.join(" ")));
                } else {
                    r.check(&format!("no gap, the command of 0.1.35: {label}"), got == want, &format!("{got} (0.1.35: {want})"));
                }
            }
            Err(e) => r.check(&format!("no gap, the command of 0.1.35: {label}"), false, &e),
        }
    }
    let _ = fs::remove_file(&png);
}

/// 0.1.36: gaps on the main video row (`Range::gap`). The test clip is 8 s, still from 3 to 5 s;
/// the desktop tone is on track 1, the microphone's on track 2; `tone` is a file (600 Hz from 1 s).
fn gap_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, (f_desk, f_mic): (u32, u32), tone: &str) {
    const W: usize = 64;
    const MID: (usize, usize, usize, usize) = (8, 6, 56, 30);
    let src_file = Path::new(&src.path);
    let rg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    let gap = |g: f64| Range { gap: g, ..Default::default() };
    let track = |index: u32| AudioEdit { source: "track".into(), index, volume: 1.0, ..Default::default() };
    let ref_desk = band_db_at(ffmpeg, src_file, 1, f_desk, 0.2, 2.8);
    let ref_mic = band_db_at(ffmpeg, src_file, 2, f_mic, 0.2, 2.8);
    let lum = |f: &Path, t: f64| grey_at(ffmpeg, f, t, W, 36).map(|v| box_mean(&v, W, MID)).unwrap_or(255.0);
    let durs = |p: &Path| -> Vec<f64> {
        let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
        let out = util::hidden_cmd(&ffprobe).args(["-v", "error", "-show_entries", "stream=duration", "-of", "csv=p=0"]).arg(p).stdin(Stdio::null()).stderr(Stdio::null()).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()).unwrap_or_default()
    };
    let exact = |p: &Path, want: f64| { let d = durs(p); !d.is_empty() && d.iter().all(|x| near(*x, want, 0.04)) };
    // Heard: within 1.5 dB of the source; silent: 40 dB under it.
    let heard = |x: Option<f64>, r0: Option<f64>| matches!((x, r0), (Some(v), Some(r0)) if near(v, r0, 1.5));
    let silent = |x: Option<f64>, r0: Option<f64>| matches!((x, r0), (Some(v), Some(r0)) if v < r0 - 40.0);
    let lit = |v: f64| v > 30.0 && v < 250.0;

    // The model's side of the interface.
    let parsed: Result<ExportOpts, _> = serde_json::from_str(r#"{"segments":[{"start":0,"end":2},{"gap":1.5},{"start":5.5,"end":7.5}],"ordered":true}"#);
    r.check("gap: a { gap } entry parses, ranges without it have none", parsed.as_ref().is_ok_and(|o| o.segments.len() == 3 && o.segments[1].gap == 1.5 && o.segments[0].gap == 0.0), &format!("{:?}", parsed.as_ref().err()));
    let ks = |segments: Vec<Range>| kept_ranges(&ExportOpts { segments, ordered: true, ..Default::default() }, 8.0).iter().map(|k| (k.a, k.b, k.gap)).collect::<Vec<_>>();
    r.check("gap: two touching ranges with a gap between them stay apart", ks(vec![rg(0.0, 2.0), gap(1.0), rg(2.0, 4.0)]) == vec![(0.0, 2.0, 0.0), (2.0, 4.0, 1.0)], &format!("{:?}", ks(vec![rg(0.0, 2.0), gap(1.0), rg(2.0, 4.0)])));
    r.check("gap: a first gap goes before the first range, one after the last is dropped, two in a row add up", ks(vec![gap(0.5), gap(0.25), rg(1.0, 2.0), gap(3.0)]) == vec![(1.0, 2.0, 0.75)], &format!("{:?}", ks(vec![gap(0.5), gap(0.25), rg(1.0, 2.0), gap(3.0)])));
    let unordered = kept_ranges(&ExportOpts { segments: vec![rg(5.0, 6.0), gap(1.0), rg(0.0, 1.0)], ..Default::default() }, 8.0);
    r.check("gap: a list with a gap keeps its order even without `ordered`", unordered.len() == 2 && unordered[0].a == 5.0 && unordered[1].gap == 1.0, "");

    // 1. Between two ranges, the clip's own tracks (the track plan): 2 + 1.5 + 2 = 5.5 s.
    if let Ok(args) = export_args(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(1.5), rg(5.5, 7.5)], ordered: true, ..Default::default() }) {
        r.line(&format!("INFO gap command: {}", args.join(" ")));
    }
    match export(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(1.5), rg(5.5, 7.5)], ordered: true, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            r.check("gap between two ranges: 5.5 s, every stream exactly", exact(f, 5.5) && titles_of_file(ffmpeg, f).len() == 3, &format!("{:?}", durs(f)));
            let (a, g, b) = (lum(f, 1.0), lum(f, 2.75), lum(f, 4.5));
            let want_b = grey_at(ffmpeg, src_file, 7.0, W, 36).map(|v| box_mean(&v, W, MID)).unwrap_or(0.0);
            r.check("gap: black in the gap, the clip before and after it", lit(a) && g < 25.0 && lit(b) && near(b, want_b, 15.0), &format!("{a:.1} / gap {g:.1} / {b:.1} (source {want_b:.1})"));
            let lv = |a0: f64, b0: f64| band_db_head(ffmpeg, f, 1, f_desk, a0, b0);
            let (before, inside, edge, after) = (lv(0.3, 1.9), lv(2.1, 3.4), lv(3.55, 3.9), lv(4.0, 5.3));
            r.check("gap, track plan: the tone is silent in the gap and back right after it (three times)", heard(before, ref_desk) && silent(inside, ref_desk) && heard(edge, ref_desk) && heard(after, ref_desk), &format!("{before:?} {inside:?} {edge:?} {after:?} ({ref_desk:?})"));
            let late = band_db_head(ffmpeg, f, 1, f_desk, 3.35, 3.48);
            r.check("gap, track plan: nothing after the gap starts early", silent(late, ref_desk), &format!("{late:?}"));
        }
        Err(e) => r.check("gap between two ranges", false, &e),
    }
    // 2. A gap first, the mix plan: 1 + 2 = 3 s, the first frame black at 0.
    match export(s, src, &ExportOpts { segments: vec![gap(1.0), rg(0.0, 2.0)], ordered: true, audio: vec![track(1)], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let first = frames_on_screen(ffmpeg, f, &[0.0])[0];
            r.check("gap first: 3 s, the first frame is black at 0", exact(f, 3.0) && first.is_some_and(|t| t.abs() < 0.01) && lum(f, 0.0) < 25.0 && lit(lum(f, 1.5)), &format!("{:?} first frame {first:?}, {:.1} then {:.1}", durs(f), lum(f, 0.0), lum(f, 1.5)));
            let lv = |a0: f64, b0: f64| band_db_head(ffmpeg, f, 0, f_desk, a0, b0);
            let (inside, edge, after) = (lv(0.05, 0.9), lv(1.05, 1.4), lv(1.5, 2.9));
            r.check("gap first, mix plan: silent in the gap, the tone from its end (three times)", silent(inside, ref_desk) && heard(edge, ref_desk) && heard(after, ref_desk), &format!("{inside:?} {edge:?} {after:?}"));
        }
        Err(e) => r.check("gap first", false, &e),
    }
    // 3. Two gaps, between touching ranges too, the microphone alone: 1 + .5 + 1 + 1 + 1 = 4.5 s.
    match export(s, src, &ExportOpts { segments: vec![rg(0.0, 1.0), gap(0.5), rg(1.0, 2.0), gap(1.0), rg(5.5, 6.5)], ordered: true, audio: vec![track(2)], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let lv = |a0: f64, b0: f64| band_db_head(ffmpeg, f, 0, f_mic, a0, b0);
            let ok = exact(f, 4.5) && lum(f, 1.25) < 25.0 && lum(f, 3.0) < 25.0 && lit(lum(f, 2.0)) && lit(lum(f, 4.0))
                && silent(lv(1.05, 1.45), ref_mic) && heard(lv(1.6, 2.4), ref_mic) && silent(lv(2.55, 3.45), ref_mic) && heard(lv(3.6, 4.4), ref_mic);
            r.check("two gaps: 4.5 s, black and silent in both, the clip between and after", ok, &format!("{:?}; mic {:?} {:?} {:?} {:?}", durs(f), lv(1.05, 1.45), lv(1.6, 2.4), lv(2.55, 3.45), lv(3.6, 4.4)));
        }
        Err(e) => r.check("two gaps", false, &e),
    }
    // 4. At 2x a gap is halved with the rest: (2 + 2 + 2) / 2 = 3 s.
    match export(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(2.0), rg(5.5, 7.5)], ordered: true, speed: 2.0, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let lv = |a0: f64, b0: f64| band_db_head(ffmpeg, f, 1, f_desk, a0, b0);
            r.check("gap at 2x: 3 s, the gap 1 s long in the result", exact(f, 3.0) && lum(f, 1.5) < 25.0 && lit(lum(f, 0.5)) && lit(lum(f, 2.5)) && silent(lv(1.05, 1.95), ref_desk) && heard(lv(2.1, 2.9), ref_desk.map(|x| x)), &format!("{:?} quiet {:?} after {:?}", durs(f), lv(1.05, 1.95), lv(2.1, 2.9)));
        }
        Err(e) => r.check("gap at 2x", false, &e),
    }
    // 5. With a tail: 2 + 1 + 1 + 1 (tail) = 5 s.
    match export(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(1.0), rg(5.5, 6.5)], ordered: true, tail: 1.0, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            r.check("gap and tail: 5 s, black in the gap and after the video", exact(f, 5.0) && lum(f, 2.5) < 25.0 && lit(lum(f, 3.5)) && lum(f, 4.6) < 25.0, &format!("{:?} {:.1} {:.1} {:.1}", durs(f), lum(f, 2.5), lum(f, 3.5), lum(f, 4.6)));
        }
        Err(e) => r.check("gap and tail", false, &e),
    }
    // 6. A canvas and an overlay picture lying over the gap.
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    let png = odir.join("gap-ov.png");
    let drawn = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black@0.0:s=360x640,format=rgba,drawbox=x=0:y=320:w=180:h=320:color=white@1.0:t=fill:replace=1", "-frames:v", "1", &png.to_string_lossy()]);
    let canvas = Canvas { width: 360.0, height: 640.0, background: "black".into(), pieces: vec![CanvasPiece { sx: 0.0, sy: 0.0, sw: 1.0, sh: 1.0, x: 0.0, y: 0.0, w: 1.0, h: 0.28125 }] };
    let ov = Overlay { png: png.to_string_lossy().into_owned(), from: 2.2, to: 2.9, ..Default::default() };
    match drawn.and_then(|_| export(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(1.0), rg(5.5, 7.5)], ordered: true, canvas: Some(canvas), overlays: vec![ov], ..Default::default() }, &mut |_| {})) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let g = |t: f64| grey_at(ffmpeg, f, t, 36, 64);
            let piece = |v: &Option<Vec<u8>>| v.as_ref().map(|v| box_mean(v, 36, (4, 2, 32, 16))).unwrap_or(255.0); // the clip's piece at the top
            let white = |v: &Option<Vec<u8>>| v.as_ref().map(|v| box_mean(v, 36, (2, 34, 16, 62))).unwrap_or(0.0); // the picture's quarter
            let (inside, before) = (g(2.5), g(1.0));
            let size = library::probe(ffmpeg, f, false).map(|p| (p.width, p.height));
            r.check("gap with a canvas and a picture over it: the canvas's size, the piece black in the gap, the picture on it", size == Some((360, 640)) && exact(f, 5.0) && piece(&inside) < 25.0 && lit(piece(&before)) && white(&inside) > 200.0 && white(&before) < 25.0, &format!("{size:?} {:?}; piece {:.1} (before {:.1}), picture {:.1} (before {:.1})", durs(f), piece(&inside), piece(&before), white(&inside), white(&before)));
        }
        Err(e) => r.check("gap with a canvas and a picture over it", false, &e),
    }
    // 7. A file that plays through cuts is heard over the gap; the microphone is not.
    let thru = AudioEdit { source: "file".into(), path: tone.into(), offset: 1.0, trim_start: 1.0, length: 3.0, volume: 1.0, through: true, ..Default::default() };
    let ref_file = band_db_at(ffmpeg, Path::new(tone), 0, 600, 1.3, 5.7);
    match export(s, src, &ExportOpts { segments: vec![rg(0.0, 2.0), gap(2.0), rg(5.5, 7.5)], ordered: true, audio: vec![track(2), thru], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let (file_gap, mic_gap, mic_after) = (band_db_head(ffmpeg, f, 0, 600, 2.2, 3.8), band_db_head(ffmpeg, f, 0, f_mic, 2.1, 3.9), band_db_head(ffmpeg, f, 0, f_mic, 4.2, 5.8));
            r.check("gap: a file that plays through is heard over it, the clip's microphone is not", exact(f, 6.0) && heard(file_gap, ref_file) && silent(mic_gap, ref_mic) && heard(mic_after, ref_mic), &format!("{:?} file {file_gap:?} ({ref_file:?}), mic {mic_gap:?} then {mic_after:?}", durs(f)));
        }
        Err(e) => r.check("gap: a file that plays through", false, &e),
    }
}

/// 0.1.41: the stall watchdog, the cancel and the kill-on-close job, with an ffmpeg that goes silent
/// (one frame, then the next a hundred seconds later in real time: `-re`), and a stall limit of 2 s.
fn watchdog_checks(r: &mut Report, ffmpeg: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    let silent = || {
        let mut cmd = util::hidden_cmd(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-re", "-f", "lavfi", "-i", "color=c=black:s=64x36:r=0.01:d=1000", "-f", "null", "-progress", "pipe:1", "-"]);
        cmd
    };
    STALL_MS_TEST.store(2000, Ordering::Relaxed);
    let t0 = Instant::now();
    let res = run_watched(silent(), 1000.0, &mut |_| {}, None);
    let secs = t0.elapsed().as_secs_f64();
    r.check("a silent ffmpeg is ended by the stall watchdog, with the message", res.as_ref().err().is_some_and(|e| e.starts_with("The export stopped making progress at ")) && secs < 8.0, &format!("{res:?} after {secs:.1} s"));
    STALL_MS_TEST.store(0, Ordering::Relaxed);
    let flag = std::sync::Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(800));
        f2.store(true, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let res = run_watched(silent(), 1000.0, &mut |_| {}, Some(&flag));
    let secs = t0.elapsed().as_secs_f64();
    r.check("a cancel ends the running ffmpeg at once", res.as_ref().err().is_some_and(|e| e == CANCELLED) && secs < 4.0, &format!("{res:?} after {secs:.1} s"));
    // cancel_export by id: false with nothing running, true while one runs.
    let none = cancel_export("no-such-export");
    let guard = ExportGuard::new("e2e-cancel");
    let some = cancel_export("e2e-cancel") && guard.flag.load(Ordering::SeqCst);
    drop(guard);
    r.check("cancel_export: false with nothing running, true for a running export", !none && some && !cancel_export("e2e-cancel"), "");
    // Every export / join / render ffmpeg is in the kill-on-close job: Windows ends it with Glint.
    let child = silent().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(mut c) => {
            util::track_child(&c);
            let tracked = util::child_is_tracked(&c);
            let _ = c.kill();
            let _ = c.wait();
            r.check("a child Glint starts is tied to Glint's life (kill-on-close job)", tracked, "");
        }
        Err(e) => r.check("a child Glint starts is tied to Glint's life", false, &e.to_string()),
    }
}

/// 0.1.41, the hang: a file whose picture changes its colour tags part way (two encodes with other
/// `-colorspace` / `-color_primaries` / `-color_trc`, joined without re-encoding), cut into ranges out of
/// order with a range that ends early, exported with a time limit. Up to 0.1.40 such an export froze.
fn tag_change_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    use std::time::Instant;
    let part = |name: &str, tags: [&str; 3], tone: u32| -> PathBuf {
        let p = dir.join(name);
        let _ = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x180:r=60:d=6", "-f", "lavfi", "-i", &format!("sine=frequency={tone}:sample_rate=48000:duration=6"),
            "-c:v", "libx264", "-preset", "ultrafast", "-g", "60", "-pix_fmt", "yuv420p", "-colorspace", tags[0], "-color_primaries", tags[1], "-color_trc", tags[2], "-c:a", "aac", "-shortest", &p.to_string_lossy()]);
        p
    };
    let a = part("tags-a.mp4", ["bt709", "bt709", "bt709"], 600);
    let b = part("tags-b.mp4", ["bt470bg", "bt709", "iec61966-2-1"], 900);
    let list = dir.join("tags-list.txt");
    let _ = fs::write(&list, format!("file '{}'\nfile '{}'\nfile '{}'\n", a.to_string_lossy().replace('\'', "'\\''"), b.to_string_lossy().replace('\'', "'\\''"), a.to_string_lossy().replace('\'', "'\\''")));
    let joined = dir.join("tags-joined.mp4");
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", &list.to_string_lossy(), "-c", "copy", &joined.to_string_lossy()]);
    let src = library::register(ffmpeg, &joined, "Tags", "Test Game", "clip");
    if made.is_err() || src.duration < 17.0 {
        r.check("tag change: test file made (18 s, tags change at 6 and 12 s)", false, &format!("{made:?} {:.1} s", src.duration));
        return;
    }
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    // Ranges like the owner's: an early one, then two where the second lies earlier in the file and in other tags.
    // An early range that ends, then ranges that run across a tag change (at 12 s and at 6 s), the second earlier in the file.
    let opts = ExportOpts { segments: vec![seg(0.5, 2.0), seg(10.5, 13.5), seg(4.0, 7.0)], ordered: true, ..Default::default() };
    let t0 = Instant::now();
    let res = export(s, &src, &opts, &mut |_| {});
    let secs = t0.elapsed().as_secs_f64();
    let ok = res.as_ref().is_ok_and(|m| (m.duration - 7.5).abs() < 0.1);
    r.check("tag change: ranges out of order across a colour-tag change export (no hang)", ok && secs < 60.0, &format!("{:?} in {secs:.1} s", res.as_ref().map(|m| m.duration)));
}

/// A clip with a white flash and a 1 kHz beep (every track) at 0.5, 1.5, 2.5 ... s: `fps`, a keyframe every
/// two seconds (cuts start between keyframes), `vfr` = frame times jittered, `audio_extra` = sound longer than
/// the picture, and Glint's own way of writing clips (90 kHz picture clock, AAC with its priming).
pub(crate) fn flash_clip(ffmpeg: &Path, out: &Path, fps: u32, secs: f64, vfr: bool, audio_extra: f64) -> bool {
    let frame = 1.0 / fps as f64;
    let mut vf = format!("color=c=black:s=160x90:r={fps}:d={secs},drawbox=c=white:t=fill:enable='lt(mod(t-0.5+{h},1),{w})'", h = frame * 0.25, w = frame * 0.85);
    if vfr {
        vf.push_str(&format!(",settb=1/90000,setpts='(N/{fps}+0.003*sin(N*1.7))/TB'"));
    }
    let beep = "if(lt(mod(t-0.5,1),0.06)*gte(t,0.5),0.8*sin(2*PI*1000*t),0)";
    let asecs = secs + audio_extra;
    let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"].map(String::from).to_vec();
    a.push(vf);
    for _ in 0..3 {
        a.extend(["-f", "lavfi", "-i"].map(String::from));
        a.push(format!("aevalsrc='{beep}|{beep}':s=48000:d={asecs}"));
    }
    a.extend(["-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:a", "-c:v", "libx264", "-preset", "ultrafast", "-bf", "0", "-pix_fmt", "yuv420p", "-g"].map(String::from));
    a.push((fps * 2).to_string());
    a.extend(["-video_track_timescale", "90000"].map(String::from));
    if vfr {
        a.extend(["-fps_mode", "passthrough"].map(String::from));
    }
    a.extend(["-c:a", "aac", "-b:a", "128k", "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop", "-metadata:s:a:2", "title=Microphone"].map(String::from));
    a.push(out.to_string_lossy().into_owned());
    let args: Vec<&str> = a.iter().map(String::as_str).collect();
    run_ffmpeg(ffmpeg, &args).is_ok()
}

/// Times of the white flashes in `file` (picture timestamps, as a player shows them).
fn flash_times(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-map", "0:v:0", "-vf", "scale=16:16,format=gray,showinfo", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stderr);
    let (mut found, mut prev) = (Vec::new(), 0u32);
    for line in text.lines() {
        let (Some(p), Some(m)) = (line.find("pts_time:"), line.find("mean:[")) else { continue };
        let pts: f64 = line[p + 9..].split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(-1.0);
        let y: u32 = line[m + 6..].split(|c: char| !c.is_ascii_digit()).next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if y > 128 && prev <= 128 {
            found.push(pts);
        }
        prev = y;
    }
    found
}

/// Starts of the beeps on the first audio track of `file`, decoded back to back from its start (as a player
/// plays it), on the file's clock (the track's start time added).
fn beep_times(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"]).arg(file).args(["-map", "0:a:0", "-ac", "1", "-ar", "48000", "-f", "s16le", "-"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output() else { return Vec::new() };
    let start = library::ffprobe_for(ffmpeg)
        .and_then(|fp| util::hidden_cmd(&fp).args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=start_time", "-of", "default=nw=1:nk=1"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .unwrap_or(0.0);
    let pcm: Vec<i16> = out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    let (mut found, mut quiet) = (Vec::new(), 48_000usize);
    for (i, &v) in pcm.iter().enumerate() {
        if v.unsigned_abs() > 3000 {
            if quiet > 4800 {
                found.push(start + i as f64 / 48_000.0);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    found
}

/// Sound minus picture at every flash of `file` that has a beep within 0.3 s; (offsets, flashes without a beep).
fn av_offsets(ffmpeg: &Path, file: &Path) -> (Vec<f64>, usize) {
    let (fl, be) = (flash_times(ffmpeg, file), beep_times(ffmpeg, file));
    let mut offs = Vec::new();
    let mut lone = 0;
    for f in &fl {
        match be.iter().map(|b| b - f).filter(|d| d.abs() < 0.3).min_by(|a, b| a.abs().total_cmp(&b.abs())) {
            Some(d) => offs.push(d),
            None => lone += 1,
        }
    }
    (offs, lone)
}

/// 0.1.41, a user's report ("the gameplay is delayed against the sound"): picture against sound, to the frame,
/// through every way an export can go, on clips that tell the time by a flash and a beep each second: 60 and
/// 30 fps, a variable frame rate, sound longer than the picture; cuts that start between keyframes.
fn sync_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    let seg = |a: f64, b: f64| Range { start: a, end: b, ..Default::default() };
    let sped = |a: f64, b: f64, x: f64| Range { start: a, end: b, speed: x, ..Default::default() };
    let warm = Color { brightness: 1.1, ..Default::default() };
    let clips = [("60 fps", 60u32, false, 0.0), ("30 fps", 30, false, 0.0), ("variable rate", 60, true, 0.0), ("sound longer than the picture", 60, false, 0.7)];
    for (name, fps, vfr, extra) in clips {
        let file = dir.join(format!("sync {name}.mp4"));
        if !flash_clip(ffmpeg, &file, fps, 12.0, vfr, extra) {
            r.check(&format!("sync: clip {name} made"), false, "");
            continue;
        }
        let src = library::register(ffmpeg, &file, &format!("Sync {name}"), "Test Game", "clip");
        let src = ClipMeta { fps, ..src };
        let tol = 1.0 / fps as f64 + 0.004;
        let (base, lone0) = av_offsets(ffmpeg, &file);
        let paths: Vec<(&str, ExportOpts)> = vec![
            ("lossless cut from 2.3 s", ExportOpts { segments: vec![seg(2.3, 9.7)], ..Default::default() }),
            ("one range re-encoded (a colour)", ExportOpts { segments: vec![seg(2.3, 9.7)], color: warm.clone(), ..Default::default() }),
            ("several ranges, one out of order", ExportOpts { segments: vec![seg(1.2, 3.8), seg(7.3, 9.9), seg(4.1, 6.6)], ordered: true, ..Default::default() }),
            ("with a gap", ExportOpts { segments: vec![seg(1.2, 3.8), Range { gap: 1.3, ..Default::default() }, seg(4.1, 6.6)], ordered: true, ..Default::default() }),
            ("with a tail", ExportOpts { segments: vec![seg(1.2, 6.6)], tail: 1.5, ..Default::default() }),
            // From a frame that 2x keeps the flashes on (0.1.44: the speed cap keeps exactly every other frame now).
            ("whole-result speed 2x", ExportOpts { segments: vec![seg(1.2 + 1.0 / 30.0, 9.6)], speed: 2.0, ..Default::default() }),
            ("one range at 2x between two at 1x", ExportOpts { segments: vec![seg(1.2, 3.7), sped(3.7, 7.7, 2.0), seg(7.7, 10.4)], ordered: true, ..Default::default() }),
            ("a range at 0.5x", ExportOpts { segments: vec![sped(1.2, 4.6, 0.5)], ..Default::default() }),
        ];
        let mut worst: Vec<String> = Vec::new();
        let mut bad = false;
        for (label, opts) in paths {
            match export(s, &src, &opts, &mut |_| {}) {
                Ok(m) => {
                    let (offs, lone) = av_offsets(ffmpeg, Path::new(&m.path));
                    // At 0.5x a beep is 120 ms but starts on its flash all the same; at 2x and in a sped range 30 ms.
                    let max = offs.iter().fold(0.0f64, |a, b| a.max(b.abs()));
                    // Slowed sound: atempo (pitch kept) stretches each sound's start, which then reads up to about 60 ms
                    // early; the picture itself is exact (measured). Everything else: within a frame.
                    let tol = if label.contains("0.5x") { 0.065 } else { tol };
                    if offs.len() < 2 || lone > 0 || max > tol {
                        bad = true;
                    }
                    if offs.len() < 2 || lone > 0 || max > tol {
                        let p = Path::new(&m.path);
                        r.line(&format!("INFO sync {name} {label}: flashes {:?} beeps {:?}", flash_times(ffmpeg, p).iter().take(6).map(|x| (x * 1000.0).round() / 1000.0).collect::<Vec<_>>(), beep_times(ffmpeg, p).iter().take(6).map(|x| (x * 1000.0).round() / 1000.0).collect::<Vec<_>>()));
                    }
                    worst.push(format!("{label}: {} flashes, worst {:+.1} ms{}", offs.len(), offs.iter().copied().max_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(f64::NAN) * 1000.0, if lone > 0 { format!(", {lone} without sound") } else { String::new() }));
                    let _ = fs::remove_file(&m.path);
                }
                Err(e) => {
                    bad = true;
                    worst.push(format!("{label}: {e}"));
                }
            }
        }
        let src_worst = base.iter().fold(0.0f64, |a, b| a.max(b.abs()));
        r.check(&format!("sync, {name}: picture and sound within a frame through every export path"), !bad && lone0 == 0 && src_worst <= tol, &format!("source worst {:.1} ms; {}", src_worst * 1000.0, worst.join("; ")));
    }
}

/// 0.1.41, "the colours change a frame before the cut": at every cut no frame is dropped, doubled or out of
/// order, and a range's colour is on exactly its own frames, at 60 fps and from a 30 fps source. The clip's left
/// half is a frame counter (grey level 3 N mod 200 + 20, lossless), its right half a constant grey 100.
fn cut_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    // (time, mean grey) of every frame of `file`, of its left or right half.
    let halves = |file: &Path, right: bool| -> Vec<(f64, f64)> {
        let crop = if right { "crop=iw/2:ih:iw/2:0" } else { "crop=iw/2:ih:0:0" };
        let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-vf", &format!("{crop},format=gray,showinfo"), "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
        String::from_utf8_lossy(&out.stderr).lines().filter_map(|l| {
            let p = l.find("pts_time:")?;
            let mm = l.find("mean:[")?;
            Some((l[p + 9..].split_whitespace().next()?.parse().ok()?, l[mm + 6..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.parse().ok()?))
        }).collect()
    };
    for fps in [60u32, 30] {
        let file = dir.join(format!("cuts {fps}.mp4"));
        let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("color=c=gray:s=320x180:r={fps}:d=8,geq=lum='if(lt(X,W/2),mod(N*3,200)+20,100)':cb=128:cr=128"), "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", &(fps * 2).to_string(), "-pix_fmt", "yuv420p", &file.to_string_lossy()]);
        if made.is_err() {
            r.check(&format!("cuts {fps} fps: clip made"), false, &format!("{made:?}"));
            continue;
        }
        let src = ClipMeta { fps, ..library::register(ffmpeg, &file, "Cuts", "Test Game", "clip") };
        let frame = 1.0 / fps as f64;
        // A plain, B brighter (its own colour), C plain again, C following on from A in the file; cuts mid-frame.
        let a = Range { start: 0.5 + frame * 0.4, end: 2.0 + frame * 0.4, ..Default::default() };
        let b = Range { start: 4.0 + frame * 0.6, end: 5.5 + frame * 0.6, color: Some(Color { brightness: 1.6, ..Default::default() }), ..Default::default() };
        let cc = Range { start: 2.0 + frame * 0.4, end: 3.0, ..Default::default() };
        let opts = ExportOpts { segments: vec![a.clone(), b.clone(), cc.clone()], ordered: true, ..Default::default() };
        let m = match export(s, &src, &opts, &mut |_| {}) {
            Ok(m) => m,
            Err(e) => {
                r.check(&format!("cuts {fps} fps: export"), false, &e);
                continue;
            }
        };
        let out = Path::new(&m.path);
        let (left, right) = (without_end_repeat(ffmpeg, out, halves(out, false)), without_end_repeat(ffmpeg, out, halves(out, true)));
        let (cut1, cut2) = (a.end - a.start, a.end - a.start + b.end - b.start);
        let in_b = |t: f64| t >= cut1 - 1e-3 && t < cut2 - 1e-3;
        // Times strictly rise and no step is longer than a frame (nothing dropped); a step at a cut that is not on a
        // frame boundary is shorter: the frames keep their own times.
        let steps: Vec<f64> = left.windows(2).map(|w| w[1].0 - w[0].0).collect();
        let rising = !steps.is_empty() && steps.iter().all(|d| *d > 1e-4 && *d < frame * 1.3);
        // The counter goes on by one frame (+3 grey) between every two frames of A and of C (B is brightened):
        // nothing doubled or dropped inside a range; A's last and C's first follow on in the file too.
        let bad_pairs: Vec<String> = left.windows(2).filter(|w| !in_b(w[0].0) && !in_b(w[1].0)).filter(|w| {
            // `format=gray` reads the counter in full range: back to the 16..235 values it was drawn in.
            let tv = |v: f64| v * 219.0 / 255.0 + 16.0;
            let d = (tv(w[1].1) - tv(w[0].1)).rem_euclid(200.0);
            (d - 3.0).abs() >= 1.5
        }).take(4).map(|w| format!("{:.4}:{:.1}->{:.4}:{:.1}", w[0].0, w[0].1, w[1].0, w[1].1)).collect();
        let counter_ok = bad_pairs.is_empty();
        // The colour: B's frames (and only those) read brighter on the constant half.
        let wrong: Vec<String> = right.iter().filter(|(t, v)| in_b(*t) != (*v > 130.0)).take(4).map(|(t, v)| format!("{t:.4} s: {v:.0}")).collect();
        let first_b = right.iter().find(|(t, _)| in_b(*t)).map(|x| x.0).unwrap_or(-1.0);
        r.check(&format!("cuts {fps} fps: no frame dropped, doubled or out of order at the cuts"), rising && counter_ok, &format!("{} frames, steps {:.4}..{:.4} s, counter jumps {bad_pairs:?}", left.len(), steps.iter().cloned().fold(f64::INFINITY, f64::min), steps.iter().cloned().fold(0.0, f64::max)));
        r.check(&format!("cuts {fps} fps: a range's colour on exactly its own frames"), wrong.is_empty() && (first_b - cut1).abs() < 1e-3, &format!("B's first frame at {first_b:.4} s (cut at {cut1:.4}); wrong: {wrong:?}"));
        let _ = fs::remove_file(&m.path);
    }
}
/// 0.1.42: ranges that share one input (`GROUP_BUDGET`, a long edit) give exactly the frames, at exactly the
/// times, that one input per range gives: frame counter clips at 60 and 30 fps, cuts mid-frame, small and large
/// gaps in the source, a range at 2x and one at 0.5x; and picture and sound stay together (flash and beep).
fn group_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    // The cuts as asked (mid-frame on purpose): sharing an input is compared with one input per range there.
    NO_SNAP.with(|n| n.set(true));
    group_checks_as_asked(r, ffmpeg, s, dir);
    NO_SNAP.with(|n| n.set(false));
}

/// 0.1.44, frame-exact cuts (`snap_to_frames`): frame counter clips at 60 and 30 fps cut mid-frame (two touching
/// ranges, one at 2x, one at 0.5x, one ending at the clip's end): across every cut the next frame comes exactly one
/// frame later (half at 2x before the cap, so one frame; two at 0.5x), every frame is the source frame for its
/// instant on the snapped cuts, the result is as long as the snapped ranges, and flash and beep stay together.
fn snap_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    let frames = |file: &Path| -> Vec<(f64, f64)> {
        let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-vf", "crop=iw/2:ih:0:0,format=gray,showinfo", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
        String::from_utf8_lossy(&out.stderr).lines().filter_map(|l| {
            let p = l.find("pts_time:")?;
            let mm = l.find("mean:[")?;
            Some((l[p + 9..].split_whitespace().next()?.parse().ok()?, l[mm + 6..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.parse().ok()?))
        }).collect()
    };
    let tv = |v: f64| v * 219.0 / 255.0 + 16.0;
    for fps in [60u32, 30] {
        let file = dir.join(format!("snap {fps}.mp4"));
        let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("color=c=gray:s=320x180:r={fps}:d=12,geq=lum='if(lt(X,W/2),mod(N*3,200)+20,100)':cb=128:cr=128"), "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", &(fps * 2).to_string(), "-pix_fmt", "yuv420p", &file.to_string_lossy()]);
        if made.is_err() {
            r.check(&format!("frame-exact cuts {fps} fps: clip made"), false, &format!("{made:?}"));
            continue;
        }
        let src = ClipMeta { fps, ..library::register(ffmpeg, &file, "Snap", "Test Game", "clip") };
        let fr = 1.0 / fps as f64;
        // (start, end, speed) in frames, cut points 0.3 to 0.7 of a frame off the grid.
        let asked = [(10.3, 50.6, 1.0), (50.6, 90.4, 2.0), (130.7, 170.2, 1.0), (200.4, 260.3, 0.5), (300.6, 330.3, 1.0), (12.0 * fps as f64 - 40.4, 12.0 * fps as f64, 1.0)];
        let segs: Vec<Range> = asked.iter().map(|&(a, b, sp)| Range { start: a * fr, end: b * fr, speed: sp, ..Default::default() }).collect();
        // Snapped: each point to its nearest frame; a 2x range then to an even number of frames (its end moves).
        let snapped: Vec<(f64, f64, f64)> = asked.iter().map(|&(a, b, sp): &(f64, f64, f64)| {
            let (a, mut b) = (a.round(), b.round());
            if sp > 1.0 {
                b = a + (((b - a) / sp).round() * sp).round();
            }
            (a * fr, b * fr, sp)
        }).collect();
        if std::env::var_os("GLINT_EDITOR_ONLY").is_some() {
            r.line(&format!("INFO snap {fps} command: {}", export_args(s, &src, &ExportOpts { segments: segs.clone(), ordered: true, ..Default::default() }).map(|a| a.join(" ")).unwrap_or_default()));
        }
        let m = match export(s, &src, &ExportOpts { segments: segs, ordered: true, ..Default::default() }, &mut |_| {}) {
            Ok(m) => m,
            Err(e) => {
                r.check(&format!("frame-exact cuts {fps} fps: export"), false, &e);
                continue;
            }
        };
        let out = Path::new(&m.path);
        let got = frames(out);
        let mut placed = 0.0;
        let map: Vec<(f64, f64, f64, f64)> = snapped.iter().map(|&(a, b, sp)| { let x = (placed, a, b, sp); placed += (b - a) / sp; x }).collect();
        let want_len = placed;
        // The repeat that stands at the very end (`end_cut`) is the last frame's duration, not a frame: left out.
        let got: Vec<(f64, f64)> = if got.last().is_some_and(|l| (l.0 - want_len).abs() < 0.0006) { got[..got.len() - 1].to_vec() } else { got };
        let wrong: Vec<String> = got.iter().filter_map(|&(t, v)| {
            let &(p, a, _, sp) = map.iter().rev().find(|m| m.0 <= t + 1e-4)?;
            let n = ((a + (t - p) * sp) * fps as f64 + 0.03).floor() as i64;
            let want = (n * 3).rem_euclid(200) as f64 + 20.0;
            ((tv(v) - want).abs() > 1.5).then(|| format!("{t:.4}: {} for {n}", ((tv(v) - 20.0) / 3.0).round()))
        }).take(4).collect();
        // Every step one frame of the result: 1/fps (also at 2x: the speed cap keeps one in two), 2/fps at 0.5x.
        let odd: Vec<String> = got.windows(2).filter(|w| {
            let sp = map.iter().rev().find(|m| m.0 <= w[0].0 + 1e-4).map(|m| m.3).unwrap_or(1.0);
            let step = if sp < 1.0 { fr / sp } else { fr };
            (w[1].0 - w[0].0 - step).abs() > 0.0004
        }).take(4).map(|w| format!("{:.4}->{:.4}", w[0].0, w[1].0)).collect();
        let lens = stream_lengths(ffmpeg, out);
        r.check(&format!("frame-exact cuts {fps} fps: across every cut the next frame is exactly one frame later"), got.len() > 100 && odd.is_empty(), &format!("{} frames; odd steps {odd:?}", got.len()));
        r.check(&format!("frame-exact cuts {fps} fps: every frame the source's frame on the snapped cuts"), wrong.is_empty(), &format!("{wrong:?}"));
        r.check(&format!("frame-exact cuts {fps} fps: as long as the snapped ranges ({want_len:.4} s), the last one slowed or not"), lens.first().is_some_and(|l| (l - want_len).abs() < fr * 0.6), &format!("{lens:?}"));
        if std::env::var_os("GLINT_EDITOR_ONLY").is_none() {
            let _ = fs::remove_file(out);
        }
    }
    // Sync: cuts mid-frame on the flash-and-beep clip, a slowed last piece.
    let file = dir.join("snap sync.mp4");
    if !flash_clip(ffmpeg, &file, 60, 10.0, false, 0.0) {
        r.check("frame-exact cuts: sync clip made", false, "");
        return;
    }
    let src = ClipMeta { fps: 60, ..library::register(ffmpeg, &file, "Snap sync", "Test Game", "clip") };
    let segs: Vec<Range> = (0..8).map(|k| Range { start: k as f64 + 0.2071, end: k as f64 + 0.8133, speed: if k == 7 { 0.5 } else { 1.0 }, ..Default::default() }).collect();
    match export(s, &src, &ExportOpts { segments: segs, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let p = Path::new(&m.path);
            let (fl, be) = (flash_times(ffmpeg, p), beep_times(ffmpeg, p));
            // The slowed last piece's beep is left out: `atempo` moves an onset by tens of ms (0.1.41, known).
            let offs: Vec<f64> = fl.iter().take(7).filter_map(|f| be.iter().map(|b| f - b).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect();
            let worst = offs.iter().fold(0.0f64, |w, x| w.max(x.abs()));
            let lens = stream_lengths(ffmpeg, p);
            let spread = lens.iter().fold(0.0f64, |w, l| w.max((l - lens[0]).abs()));
            r.check("frame-exact cuts: every flash on its beep, picture and sound equally long", fl.len() == 8 && be.len() == 8 && worst < 0.002 && spread < 0.03, &format!("worst {:.1} ms; lengths {lens:?}", worst * 1000.0));
            if std::env::var_os("GLINT_EDITOR_ONLY").is_none() {
                let _ = fs::remove_file(p);
            }
        }
        Err(e) => r.check("frame-exact cuts: sync", false, &e),
    }
}

fn group_checks_as_asked(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {    let frames = |file: &Path| -> Vec<(f64, f64)> {
        let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-vf", "crop=iw/2:ih:0:0,format=gray,showinfo", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
        String::from_utf8_lossy(&out.stderr).lines().filter_map(|l| {
            let p = l.find("pts_time:")?;
            let mm = l.find("mean:[")?;
            Some((l[p + 9..].split_whitespace().next()?.parse().ok()?, l[mm + 6..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.parse().ok()?))
        }).collect()
    };
    let both = |src: &ClipMeta, opts: &ExportOpts| -> Result<(PathBuf, PathBuf, String), String> {
        let one = export(s, src, opts, &mut |_| {})?;
        GROUP_BUDGET.with(|b| b.set(Some(1)));
        let grouped = export(s, src, opts, &mut |_| {});
        GROUP_BUDGET.with(|b| b.set(None));
        let inputs = format!("{} ranges through {} inputs", opts.segments.len(), LAST_INPUTS.with(|n| n.get()));
        Ok((PathBuf::from(one.path), PathBuf::from(grouped?.path), inputs))
    };
    for fps in [60u32, 30] {
        let file = dir.join(format!("groups {fps}.mp4"));
        let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("color=c=gray:s=320x180:r={fps}:d=30,geq=lum='if(lt(X,W/2),mod(N*3,200)+20,100)':cb=128:cr=128"), "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", &(fps * 2).to_string(), "-pix_fmt", "yuv420p", &file.to_string_lossy()]);
        if made.is_err() {
            r.check(&format!("shared inputs {fps} fps: clip made"), false, &format!("{made:?}"));
            continue;
        }
        let src = ClipMeta { fps, ..library::register(ffmpeg, &file, "Groups", "Test Game", "clip") };
        let frame = 1.0 / fps as f64;
        // 14 ranges in source order: cuts mid-frame, gaps of 0.3 s to 5 s, two touching, one at 2x, one at 0.5x.
        let mut segs = Vec::new();
        let mut at = 0.3 + frame * 0.4;
        for k in 0..14 {
            let len = 0.6 + 0.05 * k as f64;
            let speed = match k { 4 => 2.0, 9 => 0.5, _ => 1.0 };
            segs.push(Range { start: at, end: at + len, speed, ..Default::default() });
            at += len + match k { 2 => 0.0, 6 => 5.0, _ => 0.3 + frame * 0.3 };
        }
        // Each output frame must show the source frame on screen at the source instant its time maps to.
        let map: Vec<(f64, f64, f64, f64)> = {
            let mut placed = 0.0;
            segs.iter().map(|g| { let x = (placed, g.start, g.end, g.speed); placed += (g.end - g.start) / g.speed; x }).collect()
        };
        let tv = |v: f64| v * 219.0 / 255.0 + 16.0;
        let misplaced = |fr: &[(f64, f64)]| -> Vec<String> {
            fr.iter().filter_map(|&(t, v)| {
                let &(p, a, b, sk) = map.iter().rev().find(|m| m.0 <= t + 1e-4)?;
                let at = (a + (t - p) * sk).min(b);
                let n = (at * fps as f64 + 0.03).floor() as i64;
                let want = (n * 3).rem_euclid(200) as f64 + 20.0;
                ((tv(v) - want).abs() > 1.5).then(|| format!("{t:.4}: frame {} for {n}", ((tv(v) - 20.0) / 3.0).round()))
            }).take(4).collect()
        };
        let opts = ExportOpts { segments: segs, ..Default::default() };
        match both(&src, &opts) {
            Ok((one, grouped, how)) => {
                let (a, b) = (frames(&one), frames(&grouped));
                r.check(&format!("cuts {fps} fps, one input per range: each frame the source's frame at its instant"), misplaced(&a).is_empty(), &format!("{:?}", misplaced(&a)));
                r.check(&format!("cuts {fps} fps, shared inputs: each frame the source's frame at its instant"), misplaced(&b).is_empty(), &format!("{:?}", misplaced(&b)));
                // Steps between frames longer than a frame of the range (two at 0.5x). Both ways still lose the frame on
                // screen at a cut that comes less than a frame after the previous range's last one, to the speed cap
                // (task 3 of 0.1.42): sharing an input must not lose more.
                let holes = |fr: &[(f64, f64)]| -> Vec<String> {
                    fr.windows(2).filter(|w| {
                        let m = map.iter().rev().find(|m| m.0 <= w[0].0 + 1e-4).copied().unwrap_or((0.0, 0.0, 0.0, 1.0));
                        // A range that starts mid-frame opens on the frame on screen there, held until the next
                        // one the speed cap keeps (task 3 of 0.1.42 snaps cuts to frames): that step is not a loss.
                        let opening = w[0].0 - m.0 < 0.5 / fps as f64;
                        !opening && w[1].0 - w[0].0 > 1.3 / fps as f64 / m.3.min(1.0)
                    }).take(4).map(|w| format!("{:.4}->{:.4}", w[0].0, w[1].0)).collect()
                };
                r.check(&format!("shared inputs {fps} fps: no frame lost that one input per range keeps (±2 frames)"), a.len() > 100 && a.len().abs_diff(b.len()) <= 2 && holes(&b).len() <= holes(&a).len() && how.ends_with("through 1 inputs"), &format!("{} vs {} frames; {how}; holes {:?} / {:?}", a.len(), b.len(), holes(&a), holes(&b)));
                let _ = fs::remove_file(&one);
                let _ = fs::remove_file(&grouped);
            }
            Err(e) => r.check(&format!("shared inputs {fps} fps"), false, &e),
        }
    }
    // Sync: flash-and-beep clip, 12 ranges each holding one flash.
    let file = dir.join("groups sync.mp4");
    if !flash_clip(ffmpeg, &file, 60, 14.0, false, 0.0) {
        r.check("shared inputs: sync clip made", false, "");
        return;
    }
    let src = ClipMeta { fps: 60, ..library::register(ffmpeg, &file, "Groups sync", "Test Game", "clip") };
    let segs: Vec<Range> = (0..12).map(|k| Range { start: k as f64 + 0.21, end: k as f64 + 0.87, ..Default::default() }).collect();
    match both(&src, &ExportOpts { segments: segs, ..Default::default() }) {
        Ok((one, grouped, how)) => {
            for (label, file) in [("one input per range", &one), ("shared inputs", &grouped)] {
                let (fl, be) = (flash_times(ffmpeg, file), beep_times(ffmpeg, file));
                let offs: Vec<f64> = fl.iter().filter_map(|f| be.iter().map(|b| f - b).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect();
                let worst = offs.iter().fold(0.0f64, |w, x| w.max(x.abs()));
                r.check(&format!("{label}: every flash on its beep (within a frame)"), fl.len() == 12 && be.len() == 12 && worst < 1.0 / 60.0 + 0.002, &format!("{} flashes, {} beeps, worst {:.1} ms, all {:?}; {how}", fl.len(), be.len(), worst * 1000.0, offs.iter().map(|x| (x * 10000.0).round() / 10.0).collect::<Vec<_>>()));
            }
            if std::env::var_os("GLINT_EDITOR_ONLY").is_none() {
                let _ = fs::remove_file(&one);
                let _ = fs::remove_file(&grouped);
            }
        }
        Err(e) => r.check("shared inputs: sync", false, &e),
    }
    // A long montage like the e2e one: 150 pieces of 4 frames of a 30 fps clip, every other one at 0.5x, the first not the last (30 s).
    let file = dir.join("groups long.mp4");
    if !flash_clip(ffmpeg, &file, 30, 20.0, false, 0.0) {
        r.check("long montage: clip made", false, "");
        return;
    }
    let src = ClipMeta { fps: 30, ..library::register(ffmpeg, &file, "Groups long", "Test Game", "clip") };
    let f = 1.0 / 30.0;
    let segs: Vec<Range> = (0..150).map(|c| Range { start: c as f64 * 4.0 * f, end: (c + 1) as f64 * 4.0 * f, speed: if c % 2 == 0 { 0.5 } else { 1.0 }, ..Default::default() }).collect();
    let want: f64 = segs.iter().map(|g| (g.end - g.start) / g.speed).sum();
    // (One input per range is not compared here: 150 inputs is what ran ffmpeg out of memory.)
    match export(s, &src, &ExportOpts { segments: segs, ..Default::default() }, &mut |_| {}).map(|m| PathBuf::from(m.path)) {
        Ok(grouped) => {
            let how = format!("150 ranges through {} inputs", LAST_INPUTS.with(|n| n.get()));
            for (label, p) in [("shared inputs", &grouped)] {
                let lens = stream_lengths(ffmpeg, p);
                let ok = lens.len() == 4 && lens.iter().all(|l| (l - want).abs() < 0.04);
                r.check(&format!("long montage ({label}): picture and every track {want:.3} s long"), ok, &format!("{lens:?}; {how}"));
            }
            if std::env::var_os("GLINT_EDITOR_ONLY").is_none() {
                let _ = fs::remove_file(&grouped);
            }
        }
        Err(e) => r.check("long montage", false, &e),
    }
}

/// Smart rendering (0.1.46, `smartcut`) is unfinished: one frame short against the full encode on the test clips.
/// Until its checks pass it never runs in the app, whatever the setting says.
const SMART_READY: bool = false;

/// Where a range input's chain gets `hwdownload` when it is decoded on the graphics card (0.1.46).
const HWDL: &str = "\u{1}hwdl\u{1}";
/// The most range inputs decoded on the graphics card at once (each is a decoder session and its frames in video
/// memory); past it the CPU decodes.
const HW_MAX_INPUTS: usize = 16;

thread_local! {
    /// Test hook (0.1.46): 1 = the card's decoding fails at its start, 2 = it stops part way, 3 = no decoder.
    pub(crate) static HW_TEST: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// True while an export runs with the graphics card decoding (for `HW_TEST` 2).
    static HW_RUN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test hook (0.1.46): true = smart rendering may run although `SMART_READY` is false (its own checks only).
    pub(crate) static SMART_TRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test hook (0.1.46): true = no smart render (the full re-encode, to compare with).
    pub(crate) static SMART_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Test hook: whether the last export was smart-rendered.
    pub(crate) static SMART_USED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Bench (0.1.46): the CPU seconds of the last ffmpeg `run_watched` ran.
    pub(crate) static LAST_CPU: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    /// Test hook: whether the last export's pieces were decoded on the graphics card (to the end).
    pub(crate) static LAST_HW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether this export's pieces are decoded on the graphics card, else why not.
fn hw_decode_plan(ffmpeg: &Path, s: &Settings, enc: &str, file: &Path, inputs: usize) -> Result<(), String> {
    if s.hw_decode == "off" {
        return Err("the setting is off".into());
    }
    if enc != "nvenc" {
        return Err(format!("the encoder is {enc}, not NVENC"));
    }
    if inputs > HW_MAX_INPUTS {
        return Err(format!("{inputs} inputs, over {HW_MAX_INPUTS}"));
    }
    let probe = library::ffprobe_for(ffmpeg).ok_or("no ffprobe")?;
    let out = util::hidden_cmd(&probe).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,pix_fmt", "-of", "csv=p=0"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| e.to_string())?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut it = line.split(',');
    let (codec, pix) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    if !matches!(codec, "h264" | "hevc") || !matches!(pix, "yuv420p" | "yuvj420p") {
        return Err(format!("the clip is {codec} {pix}"));
    }
    if HW_TEST.with(|t| t.get()) == 3 || !hw_decoder_works(ffmpeg) {
        return Err("this PC's graphics card does not decode it".into());
    }
    Ok(())
}

/// Whether ffmpeg decodes H.264 on the graphics card here (once per run: a 4-frame clip made and decoded).
fn hw_decoder_works(ffmpeg: &Path) -> bool {
    static CACHE: Mutex<Option<bool>> = Mutex::new(None);
    if let Some(v) = CACHE.lock().ok().and_then(|c| *c) {
        return v;
    }
    let p = util::tmp_dir().join(format!("hwcheck-{}.mp4", util::new_id()));
    let _ = fs::create_dir_all(util::tmp_dir());
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=0.2", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", &p.to_string_lossy()]).is_ok();
    let ok = made && run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-hwaccel", "cuda", "-hwaccel_output_format", "cuda", "-i", &p.to_string_lossy(), "-vf", "hwdownload,format=nv12", "-f", "null", "-"]).is_ok();
    let _ = fs::remove_file(&p);
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(ok);
    }
    ok
}

/// The limiter at the end of a mix that can pass full scale (0.1.45): ceiling -1 dBFS in the FILE. The AAC encoder
/// rebuilds a limited tone up to 0.5 dB over the limiter's level (0.891 gave -0.6 dBFS peaks), so the limiter holds
/// 0.85 (-1.4 dBFS) and the encoded file peaks at -1.1. 5 ms lookahead /
/// attack, 50 ms release, no make-up gain (`level=disabled`): sound under the ceiling is untouched. `latency=1`
/// takes the lookahead's 5 ms back out (without it every sound came 5 ms late against the picture).
const LIMITER: &str = "alimiter=limit=0.85:attack=5:release=50:level=disabled:latency=1";

/// Past this many audio clips from files, each file is one input shared by its clips (0.1.44).
const SHARE_FILES_FROM: usize = 12;

/// The most range inputs one export opens (0.1.42): past it, neighbouring ranges share inputs.
const MAX_RANGE_INPUTS: usize = 40;
/// After the last range's picture (0.1.44): its last frame repeated for a moment, so `end_cut` has a repeat to keep.
const END_PAD: &str = ",tpad=stop_mode=clone:stop_duration=0.2";

/// The most ranges one input is shared by (0.1.42).
const MAX_SHARED: usize = 32;

/// `frames` (time first) without the repeat that holds the end of a joined picture (0.1.44, `end_cut`): a last
/// frame that ends within 4 ms of where it starts.
fn without_end_repeat<T: Clone>(ffmpeg: &Path, file: &Path, frames: Vec<(f64, T)>) -> Vec<(f64, T)> {
    let end = stream_lengths(ffmpeg, file).first().copied().unwrap_or(f64::INFINITY);
    match frames.last() {
        Some(l) if end - l.0 < 0.004 => frames[..frames.len() - 1].to_vec(),
        _ => frames,
    }
}

/// Frame times without that repeat.
fn end_repeat_off(ffmpeg: &Path, file: &Path, times: Vec<f64>) -> Vec<f64> {
    without_end_repeat(ffmpeg, file, times.into_iter().map(|t| (t, ())).collect()).into_iter().map(|x| x.0).collect()
}

/// Each stream's length in `file` (seconds, as ffprobe reads it), video first.
fn stream_lengths(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    let Some(probe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    let Ok(out) = util::hidden_cmd(&probe).args(["-v", "error", "-show_entries", "stream=duration", "-of", "csv=p=0"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect()
}

/// 0.1.41: `Color::sharpness` (unsharp, 5x5 luma, amount 1.5 x sharpness) on exactly the range that has it;
/// 0 is no filter at all (the same command as without the field), and never a lossless cut.
fn sharp_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta) {
    let parsed: Result<Color, _> = serde_json::from_str(r#"{"brightness":1,"contrast":1,"saturation":1,"gamma":1}"#);
    r.check("sharpness: a colour without it reads as 0 (off)", parsed.is_ok_and(|c| c.sharpness == 0.0), "");
    let seg = |a: f64, b: f64, sh: f32| Range { start: a, end: b, color: Some(Color { sharpness: sh, ..Default::default() }), ..Default::default() };
    let plain = ExportOpts { segments: vec![Range { start: 0.5, end: 2.5, ..Default::default() }, Range { start: 4.5, end: 6.5, color: Some(Color { brightness: 1.2, ..Default::default() }), ..Default::default() }], ordered: true, ..Default::default() };
    let zero = ExportOpts { segments: vec![seg(0.5, 2.5, 0.0), Range { start: 4.5, end: 6.5, color: Some(Color { brightness: 1.2, sharpness: 0.0, ..Default::default() }), ..Default::default() }], ordered: true, ..Default::default() };
    let same_cmd = matches!((export_args(s, src, &plain), export_args(s, src, &zero)), (Ok(a), Ok(b)) if args_hash(&a) == args_hash(&b));
    r.check("sharpness 0: the same command as without it", same_cmd, "");
    // One range sharpened, the next not: its frames have more detail than the source's, the other's do not.
    // (The test clip is still from 3 to 5 s: both ranges lie outside that.)
    match export(s, src, &ExportOpts { segments: vec![seg(0.5, 2.5, 1.0), seg(5.5, 7.5, 0.0)], ordered: true, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let out = Path::new(&m.path);
            // At the clip's own size: a smaller grab would average the sharpening away.
            let (w, h) = (src.width.max(64) as usize, src.height.max(36) as usize);
            let d = |file: &Path, t: f64| box_detail(&grey_at(ffmpeg, file, t, w, h), w, (w / 10, h / 10, w * 9 / 10, h * 9 / 10));
            let (sharp, src_a) = (d(out, 1.0), d(Path::new(&src.path), 1.5));
            let (plain_b, src_b) = (d(out, 3.0), d(Path::new(&src.path), 6.5));
            r.check("sharpness: the sharpened range's edges are stronger, the next range's are as they were", m.kind == "edit" && sharp > src_a * 1.3 && plain_b < src_b * 1.1, &format!("detail {sharp:.2} vs source {src_a:.2}; next range {plain_b:.2} vs {src_b:.2}"));
            let _ = fs::remove_file(&m.path);
        }
        Err(e) => r.check("sharpness: the sharpened range", false, &e),
    }
}

/// 0.1.46: the pieces decoded on the graphics card (NVIDIA). The same decoded frames (framemd5) as decoding on the
/// CPU: three ranges of a frame-counter clip (one with a colour), and a file whose colour tags change half way (the
/// owner's joined drafts); and every failure ends in a finished, identical export by the CPU with one log line: the
/// card's decoding failing at its start, stopping part way (40 %), and a PC without the decoder. Skipped (with a
/// line) on a PC without NVENC or without the card's decoder.
fn hw_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    // The setting on (its default is "off" since the 0.1.46 measurement).
    let on = Settings { hw_decode: "auto".into(), ..s.clone() };
    let s = &on;
    if pick_encoder(ffmpeg, &s.encoder) != "nvenc" || !hw_decoder_works(ffmpeg) {
        r.line("SKIP hardware decode: this PC has no NVENC or no NVIDIA decoder; the checks need one");
        return;
    }
    let md5s = |p: &Path| -> Vec<String> {
        let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"]).arg(p).args(["-map", "0:v:0", "-f", "framemd5", "-"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).output() else { return Vec::new() };
        String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with('#')).map(|l| { let f: Vec<&str> = l.split(',').map(str::trim).collect(); format!("{} {}", f.get(2).unwrap_or(&""), f.last().unwrap_or(&"")) }).collect()
    };
    let log_file = util::data_dir().join("glint.log");
    let log_len = || fs::read_to_string(&log_file).map(|t| t.len()).unwrap_or(0);
    let log_since = |from: usize| fs::read_to_string(&log_file).map(|t| t.get(from..).unwrap_or("").to_string()).unwrap_or_default();
    let clip = dir.join("hw counter.mp4");
    let tagged = dir.join("hw tags.mp4");
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=gray:s=640x360:r=60:d=8,geq=lum='if(lt(X,W/2),mod(N*3,200)+20,100)':cb=128:cr=128", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-g", "120", "-pix_fmt", "yuv420p", &clip.to_string_lossy()]).is_ok();
    // Colour tags that change half way (two encodes with other tags, joined by a stream copy at a keyframe).
    let (ta, tb, list) = (dir.join("hw ta.mp4"), dir.join("hw tb.mp4"), dir.join("hw list.txt"));
    let enc_tags = |p: &Path, tags: [&str; 3]| run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=6", "-c:v", "libx264", "-preset", "ultrafast", "-g", "30", "-pix_fmt", "yuv420p", "-colorspace", tags[0], "-color_primaries", tags[1], "-color_trc", tags[2], &p.to_string_lossy()]).is_ok();
    let tag_ok = enc_tags(&ta, ["bt709", "bt709", "bt709"]) && enc_tags(&tb, ["bt470bg", "bt709", "iec61966-2-1"])
        && fs::write(&list, format!("file '{}'\noutpoint 3\nfile '{}'\ninpoint 3\n", ta.to_string_lossy().replace('\\', "/"), tb.to_string_lossy().replace('\\', "/"))).is_ok()
        && run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", &list.to_string_lossy(), "-c", "copy", &tagged.to_string_lossy()]).is_ok();
    if !made || !tag_ok {
        r.check("hardware decode: test clips made", false, &format!("{made} {tag_ok}"));
        return;
    }
    let seg = |a: f64, b: f64, c: Option<Color>| Range { start: a, end: b, color: c, ..Default::default() };
    let cases: Vec<(&str, PathBuf, ExportOpts)> = vec![
        ("three ranges, one coloured", clip.clone(), ExportOpts { segments: vec![seg(0.5, 2.0, None), seg(3.0, 4.5, Some(Color { brightness: 1.2, ..Default::default() })), seg(5.0, 7.0, None)], ordered: true, ..Default::default() }),
        ("colour tags that change half way", tagged.clone(), ExportOpts { segments: vec![seg(1.0, 2.5, None), seg(2.5, 4.5, None), seg(4.5, 5.5, None)], ordered: true, ..Default::default() }),
    ];
    let off = Settings { hw_decode: "off".into(), ..s.clone() };
    for (name, file, opts) in &cases {
        let src = ClipMeta { fps: 60, ..library::register(ffmpeg, file, "HW", "Test Game", "clip") };
        let cpu = export(&off, &src, opts, &mut |_| {});
        let from = log_len();
        let gpu = export(s, &src, opts, &mut |_| {});
        let used = LAST_HW.with(|l| l.get());
        match (cpu, gpu) {
            (Ok(a), Ok(b)) => {
                let (ma, mb) = (md5s(Path::new(&a.path)), md5s(Path::new(&b.path)));
                let differ = ma.iter().zip(&mb).filter(|(x, y)| x != y).count();
                r.check(&format!("hardware decode, {name}: decoded on the card, the same frames as on the CPU"), used && ma.len() > 60 && ma.len() == mb.len() && differ == 0 && !log_since(from).contains("decoding on the CPU"), &format!("{} vs {} frames, {differ} differ; card used {used}", ma.len(), mb.len()));
                let _ = fs::remove_file(&a.path);
                let _ = fs::remove_file(&b.path);
            }
            (a, b) => r.check(&format!("hardware decode, {name}"), false, &format!("{:?} / {:?}", a.err(), b.err())),
        }
    }
    // Time on 1440p60 (10 s, three ranges): wall seconds and this process's children's CPU are not separated here,
    // so the wall time is what is reported.
    let big = dir.join("hw big.mp4");
    if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=2560x1440:r=60:d=10", "-c:v", "h264_nvenc", "-preset", "p4", "-g", "60", "-bf", "0", "-pix_fmt", "yuv420p", &big.to_string_lossy()]).is_ok() {
        let bsrc = ClipMeta { fps: 60, ..library::register(ffmpeg, &big, "HW big", "Test Game", "clip") };
        let o = ExportOpts { segments: vec![seg(0.5, 3.5, None), seg(4.0, 7.0, None), seg(7.5, 9.9, None)], ordered: true, ..Default::default() };
        let time = |st: &Settings| -> Option<f64> { let t0 = std::time::Instant::now(); let m = export(st, &bsrc, &o, &mut |_| {}).ok()?; let x = t0.elapsed().as_secs_f64(); let _ = fs::remove_file(&m.path); Some(x) };
        let (c1, g1) = (time(&off), time(s));
        r.line(&format!("INFO hardware decode on 10 s of 1440p60, three ranges: {c1:?} s on the CPU, {g1:?} s on the card"));
    }
    // The fallbacks: each ends in a finished export, frame for frame the CPU's, with one log line saying why.
    let (_, file, opts) = &cases[0];
    let src = ClipMeta { fps: 60, ..library::register(ffmpeg, file, "HW", "Test Game", "clip") };
    let reference = export(&off, &src, opts, &mut |_| {}).map(|m| { let v = md5s(Path::new(&m.path)); let _ = fs::remove_file(&m.path); v }).unwrap_or_default();
    for (code, name) in [(1u8, "the card fails at the start"), (2, "the card stops part way (40 %)"), (3, "a PC without the decoder")] {
        HW_TEST.with(|t| t.set(code));
        let from = log_len();
        let res = export(s, &src, opts, &mut |_| {});
        HW_TEST.with(|t| t.set(0));
        let said: Vec<String> = log_since(from).lines().filter(|l| l.contains("decoding on the CPU")).map(|l| l.split("export: ").nth(1).unwrap_or(l).to_string()).collect();
        match res {
            Ok(m) => {
                let got = md5s(Path::new(&m.path));
                r.check(&format!("hardware decode fallback, {name}: a finished export, the CPU's frames, one log line"), !reference.is_empty() && got == reference && said.len() == 1, &format!("{} frames (CPU {}); log: {said:?}", got.len(), reference.len()));
                let _ = fs::remove_file(&m.path);
            }
            Err(e) => r.check(&format!("hardware decode fallback, {name}"), false, &e),
        }
    }
}

/// 0.1.46, smart rendering (`smartcut`): against the full re-encode of the same cuts, on a clip recorded as Glint's
/// recorder records with NVENC (p4, hq, no B-frames, a keyframe a second) and on a draft joined from two of them by a
/// stream copy: the same number of frames, at the same times (0.2 ms), in the same order by the burned-in counter,
/// a clean decode (`-xerror`), flash and beep together, packets with rising, non-negative times, `+faststart` kept.
/// A libx264 recording (other SPS) and a source with B-frames are refused and exported in full. Skipped without NVENC.
fn smart_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    if pick_encoder(ffmpeg, &s.encoder) != "nvenc" {
        r.line("SKIP smart rendering: this PC has no NVENC (the heads must match the clip's encoder)");
        return;
    }
    let s = &Settings { smart_cuts: "auto".into(), ..s.clone() };
    SMART_TRY.with(|x| x.set(true));
    // The picture: left half a counter (16 grey levels, one step a frame), right half a white flash at k + 0.5 s.
    let video = "color=c=gray:s=1280x720:r=60:d=12,geq=lum='if(lt(X,W/2),mod(N,16)*12+40,if(lt(mod(T-0.5+0.004,1),0.014),235,30))':cb=128:cr=128";
    let beep = "if(lt(mod(t-0.5,1),0.06)*gte(t,0.5),0.8*sin(2*PI*1000*t),0)";
    let make = |p: &Path, enc: &[&str]| -> bool {
        let mut a: Vec<String> = ["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", video, "-f", "lavfi", "-i"].map(String::from).to_vec();
        a.push(format!("aevalsrc='{beep}|{beep}':s=48000:d=12"));
        a.extend(["-f", "lavfi", "-i"].map(String::from));
        a.push(format!("aevalsrc='{beep}|{beep}':s=48000:d=12"));
        a.extend(["-map", "0:v", "-map", "1:a", "-map", "2:a"].map(String::from));
        a.extend(enc.iter().map(|x| x.to_string()));
        a.extend(["-force_key_frames", "expr:gte(t,n_forced*1)", "-c:a", "aac", "-b:a", "160k", "-metadata:s:a:0", "title=Mix", "-metadata:s:a:1", "title=Desktop"].map(String::from));
        a.push(p.to_string_lossy().into_owned());
        let args: Vec<&str> = a.iter().map(String::as_str).collect();
        run_ffmpeg(ffmpeg, &args).is_ok()
    };
    let nv: &[&str] = &["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-bf", "0", "-rc", "vbr", "-cq", "23", "-b:v", "0", "-maxrate", "20000000", "-bufsize", "40000000", "-forced-idr", "1", "-pix_fmt", "yuv420p"];
    let rec = dir.join("smart rec.mp4");
    let x264 = dir.join("smart x264.mp4");
    let bfr = dir.join("smart bframes.mp4");
    let ok = make(&rec, nv) && make(&x264, &["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency", "-pix_fmt", "yuv420p"]) && make(&bfr, &["-c:v", "libx264", "-preset", "veryfast", "-bf", "3", "-pix_fmt", "yuv420p"]);
    // A draft: two recordings joined by a stream copy (as a project's old working file).
    let rec2 = dir.join("smart rec2.mp4");
    let draft = dir.join("smart draft.mp4");
    let list = dir.join("smart list.txt");
    let ok = ok && make(&rec2, nv) && fs::write(&list, format!("file '{}'\nfile '{}'\n", rec.to_string_lossy().replace('\\', "/"), rec2.to_string_lossy().replace('\\', "/"))).is_ok() && run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", &list.to_string_lossy(), "-map", "0", "-c", "copy", &draft.to_string_lossy()]).is_ok();
    if !ok {
        r.check("smart rendering: test clips made", false, "");
        return;
    }
    let tv = |v: f64| v * 219.0 / 255.0 + 16.0;
    let frames = |file: &Path| -> Vec<(f64, i64, bool)> {
        let half = |crop: &str| -> Vec<(f64, f64)> {
            let Ok(out) = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(file).args(["-vf", &format!("{crop},format=gray,showinfo"), "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() else { return Vec::new() };
            String::from_utf8_lossy(&out.stderr).lines().filter_map(|l| { let p = l.find("pts_time:")?; let m = l.find("mean:[")?; Some((l[p + 9..].split_whitespace().next()?.parse().ok()?, l[m + 6..].split(|c: char| !c.is_ascii_digit() && c != '.').next()?.parse().ok()?)) }).collect()
        };
        let (l, rr) = (half("crop=iw/2:ih:0:0"), half("crop=iw/2:ih:iw/2:0"));
        l.iter().zip(&rr).map(|(a, b)| (a.0, ((tv(a.1) - 40.0) / 12.0).round() as i64, b.1 > 128.0)).collect()
    };
    let clean = |file: &Path| util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-xerror", "-nostdin", "-i"]).arg(file).args(["-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().is_ok_and(|o| o.status.success() && o.stderr.is_empty());
    let packets_sane = |file: &Path| -> bool {
        let Some(fp) = library::ffprobe_for(ffmpeg) else { return false };
        let out = util::hidden_cmd(&fp).args(["-v", "error", "-select_streams", "v:0", "-show_entries", "packet=dts_time", "-of", "csv=p=0"]).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let d: Vec<f64> = out.lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect();
        !d.is_empty() && d[0] >= -1e-6 && d.windows(2).all(|w| w[1] > w[0])
    };
    let faststart = |file: &Path| fs::read(file).ok().and_then(|b| { let f = |tag: &[u8]| b.windows(4).position(|w| w == tag); Some(f(b"moov")? < f(b"mdat")?) }).unwrap_or(false);
    // Mid-GOP starts, one on a keyframe (3.0), one inside one second (6.2 .. 6.7), mid-frame asks (snapped).
    let segs = vec![(0.53, 2.41), (3.0, 4.27), (6.2, 6.7), (7.61, 9.99), (10.4, 11.9)];
    let opts = ExportOpts { segments: segs.iter().map(|&(a, b)| Range { start: a, end: b, ..Default::default() }).collect(), ordered: true, ..Default::default() };
    for (name, file) in [("a recording (NVENC)", &rec), ("a draft joined from two recordings", &draft)] {
        let src = ClipMeta { fps: 60, ..library::register(ffmpeg, file, "Smart", "Test Game", "clip") };
        SMART_OFF.with(|x| x.set(true));
        let t0 = std::time::Instant::now();
        let full = export(s, &src, &opts, &mut |_| {});
        let t_full = t0.elapsed().as_secs_f64();
        SMART_OFF.with(|x| x.set(false));
        let t1 = std::time::Instant::now();
        let smart = export(s, &src, &opts, &mut |_| {});
        let t_smart = t1.elapsed().as_secs_f64();
        let used = SMART_USED.with(|x| x.get());
        match (full, smart) {
            (Ok(a), Ok(b)) => {
                let (pa, pb) = (Path::new(&a.path), Path::new(&b.path));
                let (fa, fb) = (frames(pa), frames(pb));
                let bad: Vec<String> = fa.iter().zip(&fb).filter(|(x, y)| (x.0 - y.0).abs() > 0.0002 || x.1 != y.1 || x.2 != y.2).take(3).map(|(x, y)| format!("{x:?} vs {y:?}")).collect();
                r.check(&format!("smart rendering, {name}: smart, the same frames at the same times in the same order as the full encode"), used && fa.len() > 300 && fa.len() == fb.len() && bad.is_empty(), &format!("{} vs {} frames; differences {bad:?}; {t_full:.1} s full, {t_smart:.1} s smart", fa.len(), fb.len()));
                let (fl, be) = (flash_times(ffmpeg, pb), beep_times(ffmpeg, pb));
                let offs: Vec<f64> = fl.iter().filter_map(|f| be.iter().map(|x| f - x).min_by(|x, y| x.abs().total_cmp(&y.abs()))).collect();
                let worst = offs.iter().fold(0.0f64, |w, x| w.max(x.abs()));
                r.check(&format!("smart rendering, {name}: a clean decode, flash on beep, rising packet times, faststart"), clean(pb) && fl.len() >= 7 && worst < 0.002 && packets_sane(pb) && faststart(pb), &format!("clean {}, {} flashes worst {:.1} ms, packets {}, faststart {}", clean(pb), fl.len(), worst * 1000.0, packets_sane(pb), faststart(pb)));
                let _ = fs::remove_file(pa);
                let _ = fs::remove_file(pb);
            }
            (a, b) => r.check(&format!("smart rendering, {name}"), false, &format!("{:?} / {:?}", a.err(), b.err())),
        }
    }
    for (name, file) in [("a libx264 recording (another SPS)", &x264), ("a clip with B-frames", &bfr)] {
        let src = ClipMeta { fps: 60, ..library::register(ffmpeg, file, "Smart", "Test Game", "clip") };
        let res = export(s, &src, &opts, &mut |_| {});
        let used = SMART_USED.with(|x| x.get());
        r.check(&format!("smart rendering refused for {name}: exported in full"), res.is_ok() && !used, &format!("{:?}, smart {used}", res.as_ref().err()));
        if let Ok(m) = res {
            let _ = fs::remove_file(&m.path);
        }
    }
}

/// 0.1.46, deciding the default of `hw_decode`: 30 s of 1440p60 in 6 ranges, plain and with a still overlay and a
/// colour on two ranges, exported 3 times each way, alternating (CPU, card, CPU, ...): wall and ffmpeg CPU seconds,
/// medians. Only with GLINT_EDITOR_ONLY=hwbench.
fn hw_bench(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    let big = dir.join("bench 1440.mp4");
    if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=2560x1440:r=60:d=32", "-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000:duration=32", "-map", "0:v", "-map", "1:a", "-c:v", "h264_nvenc", "-preset", "p4", "-cq", "23", "-g", "60", "-bf", "0", "-pix_fmt", "yuv420p", "-c:a", "aac", &big.to_string_lossy()]).is_err() {
        r.check("bench clip made", false, "");
        return;
    }
    let src = ClipMeta { fps: 60, ..library::register(ffmpeg, &big, "Bench", "Test Game", "clip") };
    let png = overlay_dir().join("bench-still.png");
    let _ = fs::create_dir_all(overlay_dir());
    let seg = |a: f64, c: Option<Color>| Range { start: a, end: a + 5.0, color: c, ..Default::default() };
    let warm = Some(Color { brightness: 1.05, saturation: 1.2, ..Default::default() });
    let plain = ExportOpts { segments: (0..6).map(|k| seg(0.5 + 5.2 * k as f64, None)).collect(), ordered: true, ..Default::default() };
    let rich = |png: String| ExportOpts { segments: (0..6).map(|k| seg(0.5 + 5.2 * k as f64, if k == 1 || k == 4 { warm.clone() } else { None })).collect(), ordered: true, overlays: vec![Overlay { png, from: 2.0, to: 26.0, x: 0.05, y: 0.05, w: 0.3, h: 0.15, track: Vec::new(), media: None }], ..Default::default() };
    let off = Settings { hw_decode: "off".into(), ..s.clone() };
    let on = Settings { hw_decode: "auto".into(), ..s.clone() };
    let median = |mut v: Vec<f64>| { v.sort_by(|a, b| a.total_cmp(b)); v.get(v.len() / 2).copied().unwrap_or(f64::NAN) };
    for case in ["plain", "a still overlay and a colour on two ranges"] {
        let (mut cw, mut cc, mut gw, mut gc) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut card_used = true;
        for run in 0..6 {
            let card = run % 2 == 1;
            let o = if case == "plain" { plain.clone() } else {
                let _ = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=white@0.6:s=768x216,format=rgba", "-frames:v", "1", &png.to_string_lossy()]);
                rich(png.to_string_lossy().into_owned())
            };
            let t0 = std::time::Instant::now();
            let m = export(if card { &on } else { &off }, &src, &o, &mut |_| {});
            let wall = t0.elapsed().as_secs_f64();
            let cpu = LAST_CPU.with(|l| l.get());
            if card {
                card_used &= LAST_HW.with(|l| l.get());
            }
            r.line(&format!("INFO bench {case}, run {}: {} wall {wall:.2} s, ffmpeg CPU {cpu:.2} s{}", run + 1, if card { "card" } else { "CPU " }, if m.is_err() { " (FAILED)" } else { "" }));
            if let Ok(m) = m {
                let _ = fs::remove_file(&m.path);
            }
            if card { gw.push(wall); gc.push(cpu) } else { cw.push(wall); cc.push(cpu) }
        }
        let (a, b, c2, d) = (median(cw), median(gw), median(cc), median(gc));
        r.line(&format!("RESULT bench {case}: wall CPU {a:.2} s / card {b:.2} s ({:+.1} %); ffmpeg CPU seconds CPU {c2:.2} / card {d:.2} ({:+.1} %); card used every time: {card_used}", (b / a - 1.0) * 100.0, (d / c2 - 1.0) * 100.0));
    }
}

/// (peak dB, RMS dB) of the first audio track of `file` (`volumedetect`).
fn peak_rms(ffmpeg: &Path, file: &Path) -> Option<(f64, f64)> {
    let out = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-nostats", "-nostdin", "-i"]).arg(file).args(["-map", "0:a:0", "-af", "volumedetect", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().ok()?;
    let t = String::from_utf8_lossy(&out.stderr);
    let num = |k: &str| t.lines().find_map(|l| l.split(k).nth(1)).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<f64>().ok());
    Some((num("max_volume:")?, num("mean_volume:")?))
}

/// 0.1.45, "at 200 % it peaks and crackles": a mix that can pass full scale ends in a limiter (ceiling -1 dBFS).
/// A full-scale tone at 200 %: its peak at most -1 dBFS (+0.1) and still a sine, not a clipped wave (crest factor
/// about 3 dB, against about 1 dB for the clipped one); a quiet tone at 200 % exactly 6.02 dB over 100 % (the limiter
/// idle); two full-scale tones summed under the ceiling; a single row at 100 % has no limiter in its command.
fn limiter_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path) {
    let tone = |name: &str, expr: &str| -> Option<String> {
        let p = dir.join(name);
        run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("aevalsrc='{expr}':s=48000:d=5"), "-c:a", "pcm_s16le", &p.to_string_lossy()]).ok().map(|_| p.to_string_lossy().into_owned())
    };
    let (Some(full), Some(quiet), Some(other)) = (tone("full.wav", "0.999*sin(2*PI*1000*t)"), tone("quiet.wav", "0.1*sin(2*PI*1000*t)"), tone("other.wav", "0.999*sin(2*PI*1700*t)")) else {
        r.check("limiter: tone files made", false, "");
        return;
    };
    let row = |path: &str, volume: f32| AudioEdit { source: "file".into(), path: path.into(), offset: 0.0, trim_start: 0.0, length: 4.0, volume, ..Default::default() };
    let opts = |rows: Vec<AudioEdit>| ExportOpts { segments: vec![Range { start: 0.0, end: 4.0, ..Default::default() }], audio: rows, ..Default::default() };
    let level = |o: &ExportOpts| -> Result<(f64, f64), String> {
        let m = export(s, src, o, &mut |_| {})?;
        let v = peak_rms(ffmpeg, Path::new(&m.path)).ok_or("could not measure")?;
        let _ = fs::remove_file(&m.path);
        Ok(v)
    };
    let cmd = |o: &ExportOpts| export_args(s, src, o).map(|a| a.join(" ")).unwrap_or_default();
    r.check("limiter: a single row at 100 % has none; 200 % and two rows summed have it", !cmd(&opts(vec![row(&full, 1.0)])).contains("alimiter") && cmd(&opts(vec![row(&full, 2.0)])).contains(LIMITER) && cmd(&opts(vec![row(&full, 1.0), row(&other, 1.0)])).contains(LIMITER), "");
    match level(&opts(vec![row(&full, 2.0)])) {
        Ok((peak, rms)) => r.check("limiter: a full-scale tone at 200 %: peak at most -1 dBFS (+0.1), still a sine (crest over 2.5 dB)", peak <= -0.9 && peak - rms > 2.5, &format!("peak {peak:.2} dB, RMS {rms:.2} dB, crest {:.2} dB", peak - rms)),
        Err(e) => r.check("limiter: a full-scale tone at 200 %", false, &e),
    }
    match (level(&opts(vec![row(&quiet, 1.0)])), level(&opts(vec![row(&quiet, 2.0)]))) {
        (Ok(a), Ok(b)) => r.check("limiter: a quiet tone at 200 % is 6.02 dB over 100 % (untouched)", (b.1 - a.1 - 6.02).abs() <= 0.1, &format!("RMS {:.2} -> {:.2} dB ({:+.2})", a.1, b.1, b.1 - a.1)),
        (a, b) => r.check("limiter: a quiet tone at 100 % and 200 %", false, &format!("{:?} / {:?}", a.err(), b.err())),
    }
    match level(&opts(vec![row(&full, 1.0), row(&other, 1.0)])) {
        Ok((peak, rms)) => r.check("limiter: two full-scale tones summed stay under the ceiling", peak <= -0.9, &format!("peak {peak:.2} dB, RMS {rms:.2} dB")),
        Err(e) => r.check("limiter: two full-scale tones summed", false, &e),
    }
}

/// One frame of `file` at `t` as RGB, `w` x `h`.
fn rgb_at(ffmpeg: &Path, file: &Path, t: f64, w: usize, h: usize) -> Option<Vec<u8>> {
    let out = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"]).arg(file).args(["-frames:v", "1", "-vf", &format!("scale={w}:{h}:flags=neighbor"), "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    (out.stdout.len() == w * h * 3).then_some(out.stdout)
}

/// 0.1.45: `Color::temperature` and `Color::glow`. Temperature +1 on gray 128: R up 20 %, G up 2 %, B down 20 %
/// (B's preview: 154 / 131 / 102); 0 for both: the same command as without them; glow on a white square on black (and
/// a mid-gray block): the square's middle, far black and the gray untouched, light beside the square that falls off
/// with distance, the next range untouched; the time of a 10 s 1440p export with and without glow; and (when
/// GLINT_LOOK_PNG_DIR is set) one frame of the test picture with "Dark fantasy" and with glow 0.5 as PNG files.
fn look_tg_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, dir: &Path) {
    // Temperature on gray.
    let gray = dir.join("gray.mp4");
    if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=0x808080:s=320x180:r=30:d=3", "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "yuv444p", "-color_range", "pc", &gray.to_string_lossy()]).is_ok() {
        let src = ClipMeta { fps: 30, ..library::register(ffmpeg, &gray, "Gray", "Test Game", "clip") };
        let warm = Color { temperature: 1.0, ..Default::default() };
        match export(s, &src, &ExportOpts { segments: vec![Range { start: 0.0, end: 2.0, color: Some(warm), ..Default::default() }], ..Default::default() }, &mut |_| {}) {
            Ok(m) => {
                let px = rgb_at(ffmpeg, Path::new(&m.path), 1.0, 32, 18).unwrap_or_default();
                let avg = |c: usize| px.iter().skip(c).step_by(3).map(|v| *v as f64).sum::<f64>() / (px.len() / 3).max(1) as f64;
                let (rr, gg, bb) = (avg(0), avg(1), avg(2));
                let base = rgb_at(ffmpeg, &gray, 1.0, 32, 18).unwrap_or_default();
                let g0 = base.iter().map(|v| *v as f64).sum::<f64>() / base.len().max(1) as f64;
                r.check("temperature +1 on gray: R up 20 %, G up 2 %, B down 20 % (within 2 %)", (rr / g0 - 1.2).abs() < 0.024 && (gg / g0 - 1.02).abs() < 0.02 && (bb / g0 - 0.8).abs() < 0.016, &format!("gray {g0:.1} -> {rr:.1} / {gg:.1} / {bb:.1} (the preview: 154 / 131 / 102 from 128)"));
                let _ = fs::remove_file(&m.path);
            }
            Err(e) => r.check("temperature +1 on gray", false, &e),
        }
        let plain = ExportOpts { segments: vec![Range { start: 0.0, end: 1.0, color: Some(Color { brightness: 1.1, ..Default::default() }), ..Default::default() }, Range { start: 1.5, end: 2.5, ..Default::default() }], ordered: true, ..Default::default() };
        let zero = ExportOpts { segments: vec![Range { start: 0.0, end: 1.0, color: Some(Color { brightness: 1.1, temperature: 0.0, glow: 0.0, ..Default::default() }), ..Default::default() }, Range { start: 1.5, end: 2.5, color: Some(Color { temperature: 0.0, glow: 0.0, ..Default::default() }), ..Default::default() }], ordered: true, ..Default::default() };
        let parsed: Result<Color, _> = serde_json::from_str(r#"{"brightness":1,"contrast":1,"saturation":1,"gamma":1,"sharpness":0}"#);
        r.check("temperature and glow: a colour without them reads as 0; 0 is the same command as without them", parsed.is_ok_and(|c| c.temperature == 0.0 && c.glow == 0.0) && matches!((export_args(s, &src, &plain), export_args(s, &src, &zero)), (Ok(a), Ok(b)) if args_hash(&a) == args_hash(&b)), "");
    }
    // Glow: a white square (80 px) in the middle of black, a mid-gray block (100) at the left; range 1 with glow 0.5.
    let (w, h) = (640usize, 360usize);
    let sq = dir.join("square.mp4");
    let pic = "color=c=black:s=640x360:r=30:d=4,drawbox=x=280:y=140:w=80:h=80:c=white:t=fill,drawbox=x=40:y=140:w=80:h=80:c=0x646464:t=fill";
    if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", pic, "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "yuv444p", &sq.to_string_lossy()]).is_ok() {
        let src = ClipMeta { fps: 30, ..library::register(ffmpeg, &sq, "Square", "Test Game", "clip") };
        let glow = Color { glow: 0.5, ..Default::default() };
        let opts = ExportOpts { segments: vec![Range { start: 0.0, end: 2.0, color: Some(glow.clone()), ..Default::default() }, Range { start: 2.0, end: 4.0, ..Default::default() }], ..Default::default() };
        match export(s, &src, &opts, &mut |_| {}) {
            Ok(m) => {
                let out = Path::new(&m.path);
                let lum = |px: &[u8], x: usize, y: usize| { let i = (y * w + x) * 3; (px[i] as f64 + px[i + 1] as f64 + px[i + 2] as f64) / 3.0 };
                let (a, b) = (rgb_at(ffmpeg, out, 1.0, w, h).unwrap_or_default(), rgb_at(ffmpeg, out, 3.0, w, h).unwrap_or_default());
                let orig = rgb_at(ffmpeg, &sq, 1.0, w, h).unwrap_or_default();
                if a.len() == w * h * 3 && b.len() == a.len() && orig.len() == a.len() {
                    // Right of the square (it ends at x = 359), along its middle row.
                    let beside: Vec<f64> = [1usize, 3, 6, 10, 15, 25, 40].iter().map(|d| lum(&a, 359 + d, 180)).collect();
                    let falls = beside.windows(2).all(|p| p[1] <= p[0] + 0.5) && beside[0] > 8.0 && beside[6] < 3.0;
                    let mid = lum(&a, 320, 180);
                    let far = lum(&a, 600, 30);
                    let grayv = (lum(&a, 80, 180), lum(&orig, 80, 180));
                    let next_same = (0..w * h * 3).step_by(97).all(|i| (b[i] as i32 - orig[i] as i32).abs() <= 2);
                    r.check("glow 0.5 on a white square: its middle and far black untouched, light beside it falling off, the gray untouched", falls && mid >= 253.0 && far <= 2.0 && (grayv.0 - grayv.1).abs() <= 2.0, &format!("beside at 1/3/6/10/15/25/40 px {beside:.0?}; middle {mid:.0}, far {far:.0}, gray {:.0} (was {:.0}) (the preview: 42 at 1 px, 13 at 6 px, 0 by 15 px)", grayv.0, grayv.1));
                    r.check("glow on range 1 leaves range 2 untouched", next_same, "");
                } else {
                    r.check("glow: frames read", false, "");
                }
                let _ = fs::remove_file(out);
            }
            Err(e) => r.check("glow on a white square", false, &e),
        }
        // The two PNG frames for B's preview (the test picture as a still).
        if let Some(png_dir) = std::env::var_os("GLINT_LOOK_PNG_DIR").map(PathBuf::from) {
            let _ = fs::create_dir_all(&png_dir);
            let still = dir.join("look-test.png");
            let test_pic = "color=c=black:s=640x360:d=1,drawbox=x=280:y=140:w=80:h=80:c=white:t=fill,drawbox=x=40:y=140:w=80:h=80:c=0x646464:t=fill,drawbox=x=440:y=60:w=160:h=60:c=0xd04030:t=fill,drawbox=x=440:y=240:w=160:h=60:c=0x30a0e0:t=fill";
            let _ = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", test_pic, "-frames:v", "1", &still.to_string_lossy()]);
            let mut made = Vec::new();
            for (name, c) in [("dark-fantasy", crate::rules::look_color(s, "Dark fantasy")), ("glow-0.5", Some(Color { glow: 0.5, ..Default::default() }))] {
                let (Some(c), true) = (c, still.is_file()) else { continue };
                let f = look_filter(ffmpeg, &still, &c).unwrap_or_else(|| "null".into());
                let outp = png_dir.join(format!("{name}.png"));
                if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-i", &still.to_string_lossy(), "-vf", &f, "-frames:v", "1", &outp.to_string_lossy()]).is_ok() {
                    made.push(outp.to_string_lossy().into_owned());
                }
            }
            let _ = fs::copy(&still, png_dir.join("test-picture.png"));
            r.check("look frames written as PNG for the preview comparison", made.len() == 2, &format!("{} made", made.len()));
        }
    }
    // Time: 10 s of 1440p60 with and without glow.
    let big = dir.join("big.mp4");
    if run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=s=2560x1440:r=60:d=10", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", &big.to_string_lossy()]).is_ok() {
        let src = ClipMeta { fps: 60, ..library::register(ffmpeg, &big, "Big", "Test Game", "clip") };
        let time = |c: Color| -> Option<f64> {
            let t0 = std::time::Instant::now();
            let m = export(s, &src, &ExportOpts { segments: vec![Range { start: 0.0, end: 10.0, color: Some(c), ..Default::default() }], ..Default::default() }, &mut |_| {}).ok()?;
            let secs = t0.elapsed().as_secs_f64();
            let _ = fs::remove_file(&m.path);
            Some(secs)
        };
        let (a, b) = (time(Color { brightness: 1.05, ..Default::default() }), time(Color { brightness: 1.05, glow: 0.5, ..Default::default() }));
        r.line(&format!("INFO glow cost on 10 s of 1440p60: {a:?} s without, {b:?} s with"));
        r.check("glow: a 10 s 1440p export with glow works", b.is_some(), &format!("{a:?} / {b:?} s"));
    }
}

/// 0.1.44: many audio clips of one file (past `SHARE_FILES_FROM`) share one input: the file is on the command line
/// once, and each clip still plays exactly its own stretch at its own place (a beep a second in the file; clip j
/// takes the beep at 2.5 s and is placed at 0.6 j s: its beep must start there, to the millisecond).
fn shared_audio_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path) {
    let beeps = dir.join("beeps.mp4");
    if !flash_clip(ffmpeg, &beeps, 30, 6.0, false, 0.0) {
        r.check("shared audio inputs: beep file made", false, "");
        return;
    }
    let bp = beeps.to_string_lossy().into_owned();
    let n = SHARE_FILES_FROM + 4;
    let rows: Vec<AudioEdit> = (0..n).map(|j| AudioEdit { source: "file".into(), path: bp.clone(), offset: 0.5 + 0.4 * j as f64, trim_start: 2.45, length: 0.3, volume: 1.0, ..Default::default() }).collect();
    let opts = ExportOpts { segments: vec![Range { start: 0.0, end: 8.0, ..Default::default() }], audio: rows, ..Default::default() };
    let line = export_args(s, src, &opts).map(|a| a.join(" ")).unwrap_or_default();
    let inputs = line.matches("beeps.mp4").count();
    match export(s, src, &opts, &mut |_| {}) {
        Ok(m) => {
            let got = beep_times(ffmpeg, Path::new(&m.path));
            let want: Vec<f64> = (0..n).map(|j| 0.5 + 0.4 * j as f64 + 0.05).collect();
            let worst = want.iter().map(|w| got.iter().map(|g| (g - w).abs()).fold(f64::INFINITY, f64::min)).fold(0.0f64, f64::max);
            r.check(&format!("{n} audio clips of one file: the file is one input, each clip's beep at its place"), inputs == 1 && got.len() == n && worst < 0.002, &format!("{inputs} inputs; {} beeps, worst {:.1} ms", got.len(), worst * 1000.0));
            let _ = fs::remove_file(&m.path);
        }
        Err(e) => r.check(&format!("{n} audio clips of one file"), false, &e),
    }
}

/// 0.1.39: `AudioEdit::stream`. The test clip is a video file with three audio streams (Mix, Desktop,
/// Microphone: tones `f[0]`, `f[1]`, `f[2]`); a file row on that file mixes exactly the stream it names.
fn stream_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path, f: (u32, u32, u32)) {
    let freqs = [f.0, f.1, f.2];
    let refs: Vec<Option<f64>> = (0..3).map(|i| band_db_at(ffmpeg, Path::new(&src.path), i, freqs[i as usize], 0.5, 3.5)).collect();
    let row = |stream: u32| AudioEdit { source: "file".into(), path: src.path.clone(), stream, offset: 0.0, trim_start: 1.0, length: 3.0, volume: 1.0, through: true, ..Default::default() };
    for stream in 0..3u32 {
        match export(s, src, &ExportOpts { segments: vec![Range { start: 0.0, end: 4.0, ..Default::default() }], audio: vec![row(stream)], ..Default::default() }, &mut |_| {}) {
            Ok(m) => {
                let p = Path::new(&m.path);
                let lv: Vec<Option<f64>> = freqs.iter().map(|&fq| band_db_head(ffmpeg, p, 0, fq, 0.3, 2.7)).collect();
                let own = stream as usize;
                let ok = titles_of_file(ffmpeg, p).len() == 1
                    && matches!((refs[own], lv[own]), (Some(r0), Some(v)) if near(v, r0, 2.0))
                    && (0..3).filter(|&k| k != own).all(|k| matches!((refs[k], lv[k]), (Some(r0), Some(v)) if v < r0 - 30.0 || (freqs[k] == freqs[own])));
                r.check(&format!("stream {stream} of a video file with three audio streams: its own tone alone"), ok, &format!("levels {lv:?} (each stream alone {refs:?})"));
            }
            Err(e) => r.check(&format!("stream {stream} of a video file with three audio streams"), false, &e),
        }
    }
    // A stream the file does not have: a clear error before anything is encoded, nothing written.
    let count = |d: &Path| fs::read_dir(d).map(|x| x.count()).unwrap_or(0);
    let before = count(dir);
    let started = std::time::Instant::now();
    let bad = export(s, src, &ExportOpts { segments: vec![Range { start: 0.0, end: 4.0, ..Default::default() }], audio: vec![row(3)], ..Default::default() }, &mut |_| {});
    let ms = started.elapsed().as_millis();
    let msg = bad.as_ref().err().cloned().unwrap_or_default();
    r.check("a stream the file does not have is refused before encoding, nothing written", msg.contains("has no audio track 4") && count(dir) == before && ms < 3000, &format!("{msg:?} in {ms} ms"));
    // Old options without `stream` read it as 0.
    let old: Result<AudioEdit, _> = serde_json::from_str(r#"{"source":"file","path":"a.mp3","offset":1,"through":true}"#);
    r.check("an audio entry without `stream` is stream 0", old.is_ok_and(|e| e.stream == 0), "");
}

/// Audio stream titles of `f` ("" for none).
fn titles_of_file(ffmpeg: &Path, f: &Path) -> Vec<String> {
    audio_titles(ffmpeg, f).unwrap_or_default().into_iter().map(Option::unwrap_or_default).collect()
}

/// 0.1.35: `ExportOpts::tail`, black after the video. The test clip is 8 s (desktop `f_desk`,
/// microphone `f_mic` tones on tracks 1 and 2).
fn tail_checks(r: &mut Report, ffmpeg: &Path, s: &Settings, src: &ClipMeta, dir: &Path, (f_desk, f_mic): (u32, u32)) {
    const W: usize = 64;
    const TL: (usize, usize, usize, usize) = (2, 2, 30, 16); // the overlay's white quarter
    const BR: (usize, usize, usize, usize) = (34, 20, 62, 34); // away from it
    let src_file = Path::new(&src.path);
    let seg = |v: &[(f64, f64)]| v.iter().map(|&(start, end)| Range { start, end, ..Default::default() }).collect::<Vec<_>>();
    let track = |index: u32| AudioEdit { source: "track".into(), index, volume: 1.0, ..Default::default() };
    let ref_desk = band_db_at(ffmpeg, src_file, 1, f_desk, 0.2, 3.8);
    let ref_mic = band_db_at(ffmpeg, src_file, 2, f_mic, 0.2, 3.8);
    let dark = |f: &Option<Vec<u8>>, b| f.as_ref().map(|f| box_mean(f, W, b)).unwrap_or(255.0);
    let streams = |p: &Path| -> Vec<f64> {
        let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
        let out = util::hidden_cmd(&ffprobe).args(["-v", "error", "-show_entries", "stream=duration", "-of", "csv=p=0"]).arg(p).stdin(Stdio::null()).stderr(Stdio::null()).output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()).unwrap_or_default()
    };
    let old: Result<ExportOpts, _> = serde_json::from_str(r#"{"segments":[{"start":0,"end":4}]}"#);
    let new: Result<ExportOpts, _> = serde_json::from_str(r#"{"segments":[{"start":0,"end":4}],"tail":2.5}"#);
    r.check("tail: options without it parse as none, with it as given", old.is_ok_and(|o| o.tail == 0.0) && new.is_ok_and(|o| o.tail == 2.5), "");

    // 1. Only the clip's own tracks, cut to 0..4, 2 s of black after: every stream is 6 s, black and silent at the end.
    match export(s, src, &ExportOpts { segments: seg(&[(0.0, 4.0)]), tail: 2.0, ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let p = library::probe(ffmpeg, f, true);
            let durs = streams(f);
            let long = p.as_ref().is_some_and(|p| near(p.duration, 6.0, 0.1) && p.audio_streams == 3) && durs.len() == 4 && durs.iter().all(|d| near(*d, 6.0, 0.12));
            r.check("tail 2 s: the result is 2 s longer, every stream with it (no stream copy)", long && m.kind == "edit", &format!("{:?} streams {durs:?}", p.as_ref().map(|p| (p.duration, p.audio_streams))));
            let (inside, end, last) = (grey_at(ffmpeg, f, 2.0, W, 36), grey_at(ffmpeg, f, 4.5, W, 36), grey_at(ffmpeg, f, 5.9, W, 36));
            // The video before it is not black (the test picture moves, so a frame's exact match is not asked).
            let (lit, want) = (dark(&inside, BR), dark(&grey_at(ffmpeg, src_file, 2.0, W, 36), BR));
            r.check("tail: the last frames are black, the video before it is the clip's", dark(&end, BR) < 30.0 && dark(&last, TL) < 30.0 && dark(&last, BR) < 30.0 && want > 40.0 && near(lit, want, 15.0), &format!("black {:.1} / {:.1}, the video {lit:.1} (source {want:.1})", dark(&end, BR), dark(&last, BR)));
            // Frames at the clip's rate there (the overlays and keyframes move on).
            let frames = library::ffprobe_for(ffmpeg)
                .and_then(|fp| util::hidden_cmd(&fp).args(["-v", "error", "-select_streams", "v:0", "-read_intervals", "4.05%+1.9", "-show_entries", "packet=pts_time", "-of", "csv=p=0"]).arg(f).stdin(Stdio::null()).stderr(Stdio::null()).output().ok())
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.trim().parse::<f64>().is_ok_and(|t| t >= 4.04)).count())
                .unwrap_or(0) as f64;
            let want = 1.9 * src.fps.max(1) as f64;
            r.check("tail: black frames at the clip's frame rate", frames >= want * 0.9 && frames <= want * 1.1 + 2.0, &format!("{frames} frames in 1.9 s at {} fps", src.fps));
            let (heard, quiet) = (band_db_head(ffmpeg, f, 1, f_desk, 0.5, 3.5), band_db_head(ffmpeg, f, 1, f_desk, 4.3, 5.8));
            r.check("tail: the clip's sound is silent there", matches!((ref_desk, heard, quiet), (Some(r0), Some(h), Some(q)) if near(h, r0, 1.5) && q < r0 - 40.0), &format!("before {heard:?}, after the video {quiet:?} ({ref_desk:?})"));
        }
        Err(e) => r.check("tail 2 s: the result is 2 s longer", false, &e),
    }

    // 2. A song that plays through, from 3 s for 2.5 s, and a picture on the black from 4.5 to 5.8 s.
    let song = dir.join("Tail Song.wav");
    let made = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=700:sample_rate=48000:duration=6", "-ac", "2", "-c:a", "pcm_s16le", &song.to_string_lossy()]);
    let odir = overlay_dir();
    let _ = fs::create_dir_all(&odir);
    let png = odir.join("tail-ov.png");
    let drawn = run_ffmpeg(ffmpeg, &["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "color=c=black@0.0:s=640x360,format=rgba,drawbox=x=0:y=0:w=320:h=180:color=white@1.0:t=fill:replace=1", "-frames:v", "1", &png.to_string_lossy()]);
    if made.is_err() || drawn.is_err() {
        r.check("tail: test song and picture made", false, &format!("{made:?} {drawn:?}"));
        return;
    }
    let song_row = AudioEdit { source: "file".into(), path: song.to_string_lossy().into_owned(), offset: 3.0, length: 2.5, volume: 1.0, through: true, ..Default::default() };
    let ov = Overlay { png: png.to_string_lossy().into_owned(), from: 4.5, to: 5.8, ..Default::default() };
    let ref_song = band_db_at(ffmpeg, &song, 0, 700, 0.5, 5.5);
    match export(s, src, &ExportOpts { segments: seg(&[(0.0, 4.0)]), tail: 2.0, audio: vec![track(2), song_row], overlays: vec![ov], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let p = library::probe(ffmpeg, f, true);
            let durs = streams(f);
            r.check("tail with a song and a picture: 6 s, one track", p.as_ref().is_some_and(|p| near(p.duration, 6.0, 0.1) && p.audio_streams == 1) && durs.iter().all(|d| near(*d, 6.0, 0.12)), &format!("{durs:?}"));
            let pic = grey_at(ffmpeg, f, 5.0, W, 36);
            r.check("tail: a picture that lies after the video is on the black there", dark(&pic, TL) > 200.0 && dark(&pic, BR) < 30.0, &format!("picture {:.1}, black around it {:.1}", dark(&pic, TL), dark(&pic, BR)));
            let before_ov = grey_at(ffmpeg, f, 4.2, W, 36);
            r.check("tail: the picture is not there before its time", dark(&before_ov, TL) < 30.0, &format!("{:.1}", dark(&before_ov, TL)));
            let (in_tail, in_video) = (band_db_head(ffmpeg, f, 0, 700, 4.2, 5.4), band_db_head(ffmpeg, f, 0, 700, 3.2, 3.9));
            let after = band_db_head(ffmpeg, f, 0, 700, 5.6, 5.95);
            r.check("tail: a song that plays through is heard after the video, until its end", matches!((ref_song, in_tail, in_video, after), (Some(r0), Some(a), Some(b), Some(c)) if near(a, r0, 1.5) && near(b, r0, 1.5) && c < r0 - 30.0), &format!("in the video {in_video:?}, after it {in_tail:?}, past its end {after:?} ({ref_song:?})"));
            let (mic_v, mic_t) = (band_db_head(ffmpeg, f, 0, f_mic, 0.5, 3.5), band_db_head(ffmpeg, f, 0, f_mic, 4.3, 5.8));
            r.check("tail: the clip's microphone stops with the video", matches!((ref_mic, mic_v, mic_t), (Some(r0), Some(a), Some(b)) if near(a, r0, 1.5) && b < r0 - 40.0), &format!("{mic_v:?} then {mic_t:?} ({ref_mic:?})"));
        }
        Err(e) => r.check("tail with a song and a picture", false, &e),
    }

    // 3. At twice the speed the tail is still OUTPUT seconds: 4 s of clip -> 2 s, plus 1 s of black.
    match export(s, src, &ExportOpts { segments: seg(&[(0.0, 4.0)]), speed: 2.0, tail: 1.0, audio: vec![track(1)], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            let f = Path::new(&m.path);
            let durs = streams(f);
            let last = grey_at(ffmpeg, f, 2.6, W, 36);
            r.check("tail at 2x speed: output seconds (2 s of clip + 1 s of black)", durs.len() == 2 && durs.iter().all(|d| near(*d, 3.0, 0.12)) && dark(&last, BR) < 30.0, &format!("{durs:?}, black {:.1}", dark(&last, BR)));
        }
        Err(e) => r.check("tail at 2x speed", false, &e),
    }
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
    song_checks(r, ffmpeg, s, src, dir, f_mic, &util::data_dir().join("song-exports"));
    tail_checks(r, ffmpeg, s, src, dir, (f_desk, f_mic));
    command_checks(r, ffmpeg, s, src, &tone_s);
    sharp_checks(r, ffmpeg, s, src);
    // R3-10: a single plain piece shorter than a lossless cut takes (0.5 s) is re-encoded instead of refused.
    match export(s, src, &ExportOpts { segments: vec![Range { start: 1.0, end: 1.3, ..Default::default() }], ..Default::default() }, &mut |_| {}) {
        Ok(m) => {
            r.check("a 0.3 s plain piece exports (re-encoded, not refused)", m.kind == "edit" && (m.duration - 0.3).abs() < 0.06, &format!("{} {:.3} s", m.kind, m.duration));
            let _ = fs::remove_file(&m.path);
        }
        Err(e) => r.check("a 0.3 s plain piece exports (re-encoded, not refused)", false, &e),
    }
    // R3-8: temporary export files a killed run left are swept at start-up: by their exact names, only older ones.
    {
        let tmp = util::tmp_dir();
        let _ = fs::create_dir_all(&tmp);
        let old = tmp.join("projexport-e2e-left.mp4");
        let other = tmp.join("keep-me-e2e.mp4");
        let _ = fs::write(&old, b"x");
        let _ = fs::write(&other, b"x");
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400);
        for p in [&old, &other] {
            if let Ok(f) = fs::OpenOptions::new().write(true).open(p) {
                let _ = f.set_modified(past);
            }
        }
        let fresh = tmp.join("projexport-e2e-new.mp4");
        // Made after this run started, as a running export's file would be: never touched.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _ = fs::write(&fresh, b"x");
        let n = crate::projexport::sweep_tmp();
        r.check("start-up sweep: an old project join file goes, other files and a current one stay", n >= 1 && !old.exists() && other.exists() && fresh.exists(), &format!("{n} removed"));
        let _ = fs::remove_file(&other);
        let _ = fs::remove_file(&fresh);
    }
    gap_checks(r, ffmpeg, s, src, (f_desk, f_mic), &tone_s);
    stream_checks(r, ffmpeg, s, src, dir, (300, f_desk, f_mic));
    shared_audio_checks(r, ffmpeg, s, src, dir);
    limiter_checks(r, ffmpeg, s, src, dir);
    look_tg_checks(r, ffmpeg, s, dir);
    hw_checks(r, ffmpeg, s, dir);
    // smart_checks: not in the full run while smart rendering is unfinished (GLINT_EDITOR_ONLY=smart runs them).
    watchdog_checks(r, ffmpeg);
    tag_change_checks(r, ffmpeg, s, dir);
    sync_checks(r, ffmpeg, s, dir);
    cut_checks(r, ffmpeg, s, dir);
    group_checks(r, ffmpeg, s, dir);
    snap_checks(r, ffmpeg, s, dir);
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
    let piece = |start: f64, end: f64, color: Option<Color>, blank: bool| Range { start, end, color, blank, gap: 0.0, speed: 0.0 };
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
    let one = ex(ExportOpts { segments: vec![piece(0.5, 2.9, Some(grey.clone()), false)], ..Default::default() }).map(|(m, _)| end_repeat_off(ffmpeg, Path::new(&m.path), frame_times(ffmpeg, Path::new(&m.path))));
    let two = ex(ExportOpts { segments: vec![piece(0.5, 2.0, None, false), piece(2.0, 2.9, Some(grey.clone()), false)], ..Default::default() });
    match (one, two) {
        (Ok(one), Ok((m, p))) => {
            let f = Path::new(&m.path);
            let t = end_repeat_off(ffmpeg, f, frame_times(ffmpeg, f));
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
    let piece = |start: f64, end: f64, color: Option<Color>, blank: bool| Range { start, end, color, blank, gap: 0.0, speed: 0.0 };
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
    let n = crate::edcache::titles(src_path, || audio_titles(&ffmpeg, src_path))?.len(); // P-HOOK 0.1.41 (edcache.rs)
    let _slot = crate::edcache::slot_light()?; // P-HOOK 0.1.41: a few at once (after the cache check above), cancellable
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
    let _slot = crate::edcache::slot()?; // P-HOOK 0.1.41 (edcache.rs): film strips 2 at once (after the cache check), cancellable
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
