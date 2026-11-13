//! "Edit together": join several clips, oldest first, into ONE new clip and
//! remember where each of them starts, so the editor can open it with a split
//! at every join. The source clips are never changed.
//!
//! It is a background job of kind "join" (its own lane in jobs.rs). The job:
//! 1. probes every clip (streams, start times, packet counts from the header,
//!    the first packet of each stream);
//! 2. picks one of three ways (`Mode`):
//!    - "copy": every clip has the same video (codec, profile, level, size,
//!      pixel format, aspect) and the same audio (track count, codec, rate,
//!      channels). ffmpeg's concat demuxer joins them with `-c copy`. The
//!      `duration` of every file in the list is chosen so the timestamps of
//!      every stream stay monotonic and, as far as that allows, the audio stays
//!      on the picture. AAC frames are played back to back, so each file's
//!      audio really lasts its packet count x 1024 samples (priming and padding
//!      included), not its nominal length; `plan` follows that drift join by
//!      join and only takes this way when it stays within `MAX_DRIFT` (40 ms);
//!    - "copy-video": the video is the same but the audio is not (or would
//!      drift): the video is still copied through the concat demuxer, and the
//!      audio is decoded and laid out again, each clip's sound trimmed or padded
//!      with silence to exactly its picture's length, then encoded to AAC. Sync
//!      is exact by construction and only the (cheap) audio is re-encoded;
//!    - "encode": the pictures differ. Everything is re-encoded: the largest
//!      width x height of the clips, the others scaled to fit and letterboxed
//!      (never stretched, sample aspect ratio honoured), the highest frame
//!      rate, H.264 with the editor's encoder choice (hardware, libx264 when it
//!      fails), a keyframe at every join.
//!    Audio of the last two ways: the same number of tracks in every clip keeps
//!    the tracks (titles of the first clip); otherwise one stereo track made
//!    from each clip's first track (the mix). A clip without audio gives silence.
//! 3. writes `.<name>.joining` beside the first clip (deleted on any failure or
//!    cancel), then renames it to `<first title> + <n> more.mp4`;
//! 4. measures where each clip starts in the new file (the keyframe nearest
//!    to the planned join), registers it (kind "joined", the first clip's game
//!    and folder, `joins` = those times) and emits
//!    "clip-joined" { clip, joins, mode, secs }.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::library::{self, ClipMeta};
use crate::util;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

/// Most clips joined at once.
pub const MAX_CLIPS: usize = 20;
/// Most seconds of clips joined at once (by the library's lengths).
pub const MAX_TOTAL_SECS: f64 = 1800.0;
/// Largest audio/video drift at any join that "copy" may leave, seconds.
pub const MAX_DRIFT: f64 = 0.040;
/// Returned when the job was cancelled.
pub const CANCELLED: &str = "Cancelled";
/// Samples per AAC frame (AAC-LC).
const AAC_FRAME: f64 = 1024.0;
/// Room left between two files' timestamps in the concat list, seconds (rounding to the stream time bases).
const GAP: f64 = 0.0002;
/// Audio of re-encoded tracks.
const AUDIO_KBPS: u32 = 160;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

// ---------------------------------------------------------------------------
// Probing

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VInfo {
    pub codec: String,
    pub profile: String,
    pub level: i64,
    pub width: u32,
    pub height: u32,
    pub pix_fmt: String,
    /// "1:1" when unset.
    pub sar: String,
    pub has_b: i64,
    pub field_order: String,
    /// The concat demuxer does not convert time bases between files, so a copy needs them equal.
    pub time_base: String,
    /// Average frame rate (0 = unknown).
    pub fps: f64,
    /// Time of the first frame and the end of the last one (its start + its length), seconds.
    pub first: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default)]
pub struct AInfo {
    pub codec: String,
    pub profile: String,
    pub rate: u32,
    pub channels: u32,
    pub time_base: String,
    /// Time of the first packet (AAC priming makes it negative, about -0.021 s).
    pub first: f64,
    /// Packets in the stream, from the header (0 = unknown).
    pub frames: u64,
    pub title: String,
}

impl AInfo {
    /// How long the stream lasts when its packets are played back to back
    /// (AAC: priming and padding included). None when that cannot be known.
    fn played(&self) -> Option<f64> {
        (self.codec == "aac" && self.frames > 0 && self.rate > 0).then(|| self.frames as f64 * AAC_FRAME / self.rate as f64)
    }
    fn same(&self, o: &AInfo) -> bool {
        self.codec == o.codec && self.profile == o.profile && self.rate == o.rate && self.channels == o.channels && self.time_base == o.time_base
    }
}

#[derive(Debug, Clone, Default)]
pub struct Src {
    pub path: PathBuf,
    pub title: String,
    /// The file's start time (the earliest stream start, as the concat demuxer counts it).
    pub start: f64,
    pub duration: f64,
    pub v: Option<VInfo>,
    pub a: Vec<AInfo>,
}

impl Src {
    fn video(&self) -> &VInfo {
        static NONE: std::sync::OnceLock<VInfo> = std::sync::OnceLock::new();
        self.v.as_ref().unwrap_or_else(|| NONE.get_or_init(VInfo::default))
    }
    /// Length of the picture, seconds.
    fn vspan(&self) -> f64 {
        let v = self.video();
        (v.end - v.first).max(0.0)
    }
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}
fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}
fn rate(v: &Value) -> f64 {
    let s = v.as_str().unwrap_or("");
    match s.split_once('/') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<f64>().unwrap_or(0.0), d.parse::<f64>().unwrap_or(0.0));
            if d > 0.0 { n / d } else { 0.0 }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

fn ffprobe_out(ffprobe: &Path, args: &[&str], file: &Path) -> Option<String> {
    let out = util::hidden_cmd(ffprobe).args(args).arg(file).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Read what `plan` needs about one clip.
pub fn probe(ffprobe: &Path, file: &Path, title: &str) -> Result<Src, String> {
    let entries = "stream=index,codec_type,codec_name,profile,level,width,height,pix_fmt,sample_aspect_ratio,has_b_frames,field_order,avg_frame_rate,time_base,start_time,duration,nb_frames,sample_rate,channels:stream_tags=title,name:format=start_time,duration";
    let text_json = ffprobe_out(ffprobe, &["-v", "error", "-show_entries", entries, "-of", "json"], file).ok_or_else(|| format!("Could not read \"{title}\""))?;
    let j: Value = serde_json::from_str(&text_json).map_err(|_| format!("Could not read \"{title}\""))?;
    let fmt = &j["format"];
    let mut src = Src { path: file.to_path_buf(), title: title.to_string(), start: num(&fmt["start_time"]), duration: num(&fmt["duration"]), ..Default::default() };
    // stream index -> (is video, position in src.a)
    let mut kinds: Vec<(i64, bool, usize)> = Vec::new();
    for st in j["streams"].as_array().cloned().unwrap_or_default() {
        let index = st["index"].as_i64().unwrap_or(-1);
        let start = num(&st["start_time"]);
        let dur = if st["duration"].is_null() { src.duration - start } else { num(&st["duration"]) };
        match st["codec_type"].as_str() {
            Some("video") if src.v.is_none() => {
                let sar = text(&st["sample_aspect_ratio"]);
                src.v = Some(VInfo {
                    codec: text(&st["codec_name"]),
                    profile: text(&st["profile"]),
                    level: st["level"].as_i64().unwrap_or(0),
                    width: st["width"].as_u64().unwrap_or(0) as u32,
                    height: st["height"].as_u64().unwrap_or(0) as u32,
                    pix_fmt: text(&st["pix_fmt"]),
                    sar: if sar.is_empty() || sar == "0:1" { "1:1".into() } else { sar },
                    has_b: st["has_b_frames"].as_i64().unwrap_or(0),
                    field_order: text(&st["field_order"]),
                    time_base: text(&st["time_base"]),
                    fps: rate(&st["avg_frame_rate"]),
                    first: start,
                    end: start + dur,
                });
                kinds.push((index, true, 0));
            }
            Some("audio") => {
                kinds.push((index, false, src.a.len()));
                src.a.push(AInfo {
                    codec: text(&st["codec_name"]),
                    profile: text(&st["profile"]),
                    rate: num(&st["sample_rate"]) as u32,
                    channels: st["channels"].as_u64().unwrap_or(0) as u32,
                    time_base: text(&st["time_base"]),
                    first: start,
                    frames: num(&st["nb_frames"]) as u64,
                    // An MP4 track's title reads back as "name" with newer ffmpeg builds.
                    title: [text(&st["tags"]["title"]), text(&st["tags"]["name"])].into_iter().find(|s| !s.trim().is_empty()).unwrap_or_default(),
                });
            }
            _ => {}
        }
    }
    // The first packet of each stream: AAC's priming packet starts before the stream's nominal start.
    if let Some(csv) = ffprobe_out(ffprobe, &["-v", "error", "-read_intervals", "%+3", "-show_entries", "packet=stream_index,pts_time", "-of", "csv=p=0"], file) {
        let mut lowest: Vec<(i64, f64)> = Vec::new();
        for line in csv.lines() {
            let mut it = line.split(',');
            let (Some(i), Some(p)) = (it.next().and_then(|s| s.trim().parse::<i64>().ok()), it.next().and_then(|s| s.trim().parse::<f64>().ok())) else { continue };
            match lowest.iter_mut().find(|(idx, _)| *idx == i) {
                Some(e) => e.1 = e.1.min(p),
                None => lowest.push((i, p)),
            }
        }
        for (i, p) in lowest {
            match kinds.iter().find(|(idx, _, _)| *idx == i) {
                Some(&(_, true, _)) => {
                    if let Some(v) = src.v.as_mut() {
                        v.first = p;
                    }
                }
                Some(&(_, false, k)) => src.a[k].first = p,
                None => {}
            }
        }
    }
    Ok(src)
}

// ---------------------------------------------------------------------------
// Planning

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Copy,
    CopyVideo,
    Encode,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Copy => "copy",
            Mode::CopyVideo => "copy-video",
            Mode::Encode => "encode",
        }
    }
}

/// The audio of the result.
#[derive(Debug, Clone, PartialEq)]
pub enum Tracks {
    None,
    /// Track k of every clip becomes track k (titles of the first clip).
    Keep(Vec<String>),
    /// One stereo track from every clip's first track.
    Mix,
}

impl Tracks {
    fn titles(&self) -> Vec<String> {
        match self {
            Tracks::None => Vec::new(),
            Tracks::Keep(t) => t.clone(),
            Tracks::Mix => vec!["Mix".into()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub mode: Mode,
    pub tracks: Tracks,
    /// `duration` of every file but the last in the concat list (copy ways).
    pub durs: Vec<f64>,
    /// Length of every clip's piece in the result (the audio is cut / padded to it).
    pub lens: Vec<f64>,
    /// Where each clip's first frame lands in the result (planned), seconds.
    pub starts: Vec<f64>,
    /// Largest audio/video drift at any join of a straight copy (seconds, signed: negative = sound early).
    pub drift: f64,
    /// Result picture (encode).
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub total: f64,
}

fn same_video(a: &VInfo, b: &VInfo) -> bool {
    a.codec == b.codec && a.profile == b.profile && a.level == b.level && a.width == b.width && a.height == b.height && a.pix_fmt == b.pix_fmt && a.sar == b.sar && a.has_b == b.has_b && a.field_order == b.field_order && a.time_base == b.time_base
}

fn sar_value(sar: &str) -> f64 {
    match sar.split_once(':') {
        Some((n, d)) => {
            let (n, d) = (n.parse::<f64>().unwrap_or(1.0), d.parse::<f64>().unwrap_or(1.0));
            if n > 0.0 && d > 0.0 { n / d } else { 1.0 }
        }
        None => 1.0,
    }
}

/// The plan for `srcs` (already in order). `fps_hint[i]` is the library's frame
/// rate of clip i (the capture setting; 0 = unknown), used for the "encode" rate.
pub fn plan(srcs: &[Src], fps_hint: &[u32]) -> Result<Plan, String> {
    if srcs.len() < 2 {
        return Err("Pick at least two clips to edit together".into());
    }
    for s in srcs {
        let ok = s.v.as_ref().is_some_and(|v| v.width > 0 && v.height > 0 && v.end > v.first);
        if !ok {
            return Err(format!("\"{}\" has no picture", s.title));
        }
    }
    let n = srcs.len();
    let counts: Vec<usize> = srcs.iter().map(|s| s.a.len()).collect();
    let tracks = if counts.iter().all(|&c| c == 0) {
        Tracks::None
    } else if counts.iter().all(|&c| c == counts[0]) {
        Tracks::Keep(srcs[0].a.iter().enumerate().map(|(k, a)| if a.title.trim().is_empty() { format!("Track {}", k + 1) } else { a.title.clone() }).collect())
    } else {
        Tracks::Mix
    };
    let video_same = srcs.iter().all(|s| same_video(s.video(), srcs[0].video()));
    let audio_same = counts.iter().all(|&c| c == counts[0]) && (0..counts[0]).all(|k| srcs.iter().all(|s| s.a[k].same(&srcs[0].a[k])));
    let lens: Vec<f64> = srcs.iter().map(Src::vspan).collect();

    // A straight copy: the smallest `duration` per file that keeps every stream monotonic,
    // and the drift that leaves (it can only grow where a picture outlasts its sound).
    let mut drift = 0.0f64;
    if video_same && audio_same && srcs.iter().all(|s| s.a.iter().all(|a| a.played().is_some())) {
        let tracks_n = counts[0];
        let mut e = vec![0.0f64; tracks_n];
        let mut durs = Vec::with_capacity(n - 1);
        for i in 0..n - 1 {
            let (s, nx) = (&srcs[i], &srcs[i + 1]);
            let shift = nx.start - s.start;
            let mut d = (s.video().end - s.start) - (nx.video().first - nx.start);
            for k in 0..tracks_n {
                d = d.max(s.a[k].played().unwrap_or(0.0) + s.a[k].first - nx.a[k].first + shift);
            }
            d += GAP;
            for k in 0..tracks_n {
                e[k] += s.a[k].played().unwrap_or(0.0) - d + s.a[k].first - nx.a[k].first + shift;
                if e[k].abs() > drift.abs() {
                    drift = e[k];
                }
            }
            durs.push(d);
        }
        if drift.abs() <= MAX_DRIFT {
            let lift = lift(&srcs[0]);
            let mut starts = Vec::with_capacity(n);
            let mut at = 0.0;
            for (i, s) in srcs.iter().enumerate() {
                starts.push(at + s.video().first - s.start + lift);
                if i < n - 1 {
                    at += durs[i];
                }
            }
            let total = starts[n - 1] + lens[n - 1];
            return Ok(Plan { mode: Mode::Copy, tracks, durs, lens, starts, drift, width: 0, height: 0, fps: 0, total });
        }
    }
    if video_same {
        return Ok(plan_copy_video(srcs, &Plan { mode: Mode::CopyVideo, tracks, durs: Vec::new(), lens, starts: Vec::new(), drift, width: 0, height: 0, fps: 0, total: 0.0 }));
    }
    // Encode: the largest picture (as shown, sample aspect ratio applied), the highest frame rate.
    let even = |v: f64| (((v / 2.0).round() as u32) * 2).max(2);
    let (mut width, mut height) = (0u32, 0u32);
    for s in srcs {
        let v = s.video();
        let (w, h) = (even(v.width as f64 * sar_value(&v.sar)), even(v.height as f64));
        if (w as u64) * (h as u64) > (width as u64) * (height as u64) {
            (width, height) = (w, h);
        }
    }
    let fps = srcs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let hint = fps_hint.get(i).copied().unwrap_or(0);
            if hint > 0 { hint } else { s.video().fps.round() as u32 }
        })
        .max()
        .unwrap_or(60)
        .clamp(1, 240);
    let f = fps as f64;
    let mut starts = Vec::with_capacity(n);
    let mut at = 0.0;
    for &l in &lens {
        starts.push(at);
        // A piece lasts as long as its longest stream: the sound is exactly `l`, the picture whole frames.
        at += l.max((l * f).round() / f);
    }
    let total = starts[n - 1] + lens[n - 1];
    Ok(Plan { mode: Mode::Encode, tracks, durs: Vec::new(), lens, starts, drift, width, height, fps, total })
}

// ---------------------------------------------------------------------------
// ffmpeg arguments

fn sv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// The concat demuxer's list: every file, with the `duration` the plan chose.
fn concat_list(srcs: &[Src], durs: &[f64]) -> String {
    let mut s = String::from("ffconcat version 1.0\n");
    for (i, src) in srcs.iter().enumerate() {
        let p = src.path.to_string_lossy().replace('\'', "'\\''");
        s.push_str(&format!("file '{p}'\n"));
        if let Some(d) = durs.get(i) {
            s.push_str(&format!("duration {d:.6}\n"));
        }
    }
    s
}

fn f6(x: f64) -> String {
    format!("{:.6}", x.max(0.0))
}

/// Filter chain for track `k` of clip `i` (input `input`): the sound under the
/// clip's picture, from the picture's first frame, cut or padded to `len`.
fn audio_chain(src: &Src, input: usize, track: Option<usize>, len: f64, label: &str) -> String {
    match track {
        Some(k) => {
            // Decoded audio starts at 0 (the priming is skipped); the picture at `first - start`.
            let from = (src.video().first - src.start).max(0.0);
            format!(
                "[{input}:a:{k}]aresample=48000:async=1:first_pts=0,aformat=sample_fmts=fltp:channel_layouts=stereo,atrim=start={}:end={},asetpts=PTS-STARTPTS,apad=whole_dur={},atrim=end={}[{label}]",
                f6(from), f6(from + len), f6(len), f6(len)
            )
        }
        None => format!("anullsrc=r=48000:cl=stereo,atrim=end={}[{label}]", f6(len)),
    }
}

/// Which track of clip `i` feeds output track `k`.
fn source_track(tracks: &Tracks, src: &Src, k: usize) -> Option<usize> {
    match tracks {
        Tracks::None => None,
        Tracks::Keep(_) => (k < src.a.len()).then_some(k),
        Tracks::Mix => (!src.a.is_empty()).then_some(0),
    }
}

fn head() -> Vec<String> {
    sv(&["-hide_banner", "-nostdin", "-loglevel", "warning", "-nostats", "-progress", "pipe:1", "-y"])
}

fn tail(a: &mut Vec<String>, tracks: &Tracks, title: &str, out: &Path) {
    for (k, name) in tracks.titles().iter().enumerate() {
        a.push(format!("-metadata:s:a:{k}"));
        a.push(format!("title={name}"));
        a.push(format!("-disposition:a:{k}"));
        a.push(if k == 0 { "default".into() } else { "0".into() });
    }
    a.extend(sv(&["-map_chapters", "-1", "-map_metadata", "-1"]));
    a.push("-metadata".into());
    a.push(format!("title={title}"));
    a.extend(sv(&["-sn", "-dn", "-movflags", "+faststart", "-f", "mp4"]));
    a.push(out.to_string_lossy().into_owned());
}

fn audio_codec(a: &mut Vec<String>) {
    a.extend(sv(&["-c:a", "aac", "-b:a"]));
    a.push(format!("{AUDIO_KBPS}k"));
    a.extend(sv(&["-ar", "48000", "-ac", "2"]));
}

/// Arguments for the copy ways (input 0 = the concat list).
fn copy_args(srcs: &[Src], plan: &Plan, list: &Path, title: &str, out: &Path) -> Vec<String> {
    let mut a = head();
    a.extend(sv(&["-f", "concat", "-safe", "0", "-i"]));
    a.push(list.to_string_lossy().into_owned());
    let outs = plan.tracks.titles().len();
    if plan.mode == Mode::CopyVideo && outs > 0 {
        for s in srcs {
            a.push("-i".into());
            a.push(s.path.to_string_lossy().into_owned());
        }
        let mut g = Vec::new();
        let mut cat = String::new();
        for (i, s) in srcs.iter().enumerate() {
            for k in 0..outs {
                let label = format!("a{i}_{k}");
                g.push(audio_chain(s, i + 1, source_track(&plan.tracks, s, k), plan.lens[i], &label));
                cat.push_str(&format!("[{label}]"));
            }
        }
        let lift = plan.starts[0];
        let outs_l: String = (0..outs).map(|k| format!("[c{k}]")).collect();
        g.push(format!("{cat}concat=n={}:v=0:a={outs}{outs_l}", srcs.len()));
        for k in 0..outs {
            g.push(format!("[c{k}]asetpts=PTS-STARTPTS+{}/TB[ao{k}]", f6(lift)));
        }
        a.push("-filter_complex".into());
        a.push(g.join(";"));
        a.extend(sv(&["-map", "0:v:0"]));
        for k in 0..outs {
            a.push("-map".into());
            a.push(format!("[ao{k}]"));
        }
        a.extend(sv(&["-c:v", "copy"]));
        audio_codec(&mut a);
    } else {
        a.extend(sv(&["-map", "0:v:0"]));
        if plan.mode == Mode::Copy {
            for k in 0..outs {
                a.push("-map".into());
                a.push(format!("0:a:{k}"));
            }
        }
        a.extend(sv(&["-c", "copy"]));
    }
    tail(&mut a, &plan.tracks, title, out);
    a
}

/// Arguments for "encode" with encoder `enc` ("nvenc" | "amf" | "qsv" | "x264").
fn encode_args(srcs: &[Src], plan: &Plan, enc: &str, title: &str, out: &Path) -> Vec<String> {
    let mut a = head();
    for s in srcs {
        a.push("-i".into());
        a.push(s.path.to_string_lossy().into_owned());
    }
    let (w, h, fps) = (plan.width, plan.height, plan.fps);
    let outs = plan.tracks.titles().len();
    let mut g = Vec::new();
    let mut cat = String::new();
    for (i, s) in srcs.iter().enumerate() {
        let sar = if sar_value(&s.video().sar) != 1.0 { ",scale=w='trunc(iw*sar/2)*2':h=ih,setsar=1" } else { "" };
        g.push(format!(
            "[{i}:v:0]setpts=PTS-STARTPTS{sar},scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2:flags=bicubic,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,fps={fps},format=yuv420p[v{i}]"
        ));
        cat.push_str(&format!("[v{i}]"));
        for k in 0..outs {
            let label = format!("a{i}_{k}");
            g.push(audio_chain(s, i, source_track(&plan.tracks, s, k), plan.lens[i], &label));
            cat.push_str(&format!("[{label}]"));
        }
    }
    let outs_l: String = (0..outs).map(|k| format!("[ao{k}]")).collect();
    g.push(format!("{cat}concat=n={}:v=1:a={outs}[vo]{outs_l}", srcs.len()));
    a.push("-filter_complex".into());
    a.push(g.join(";"));
    a.extend(sv(&["-map", "[vo]"]));
    for k in 0..outs {
        a.push("-map".into());
        a.push(format!("[ao{k}]"));
    }
    a.extend(crate::editor::video_codec_args(enc));
    a.extend(sv(&["-pix_fmt", "yuv420p"]));
    // A keyframe where every clip starts: the joins can be cut without re-encoding.
    let keys: Vec<String> = plan.starts.iter().skip(1).map(|t| format!("{t:.3}")).collect();
    if !keys.is_empty() {
        a.push("-force_key_frames".into());
        a.push(keys.join(","));
    }
    if outs > 0 {
        audio_codec(&mut a);
    }
    tail(&mut a, &plan.tracks, title, out);
    a
}

// ---------------------------------------------------------------------------
// Running

/// Run ffmpeg (`-progress pipe:1`), reporting 0..1 of `expected` seconds done.
/// Kills it within about 0.1 s once `cancel` is set and returns `CANCELLED`.
/// On success returns what ffmpeg wrote to stderr (its warnings).
fn run_ffmpeg(ffmpeg: &Path, args: &[String], expected: f64, cancel: &AtomicBool, on_frac: &mut dyn FnMut(f64)) -> Result<String, String> {
    let mut child = Command::new(ffmpeg)
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start ffmpeg: {e}"))?;
    let err_reader = child.stderr.take().map(|mut err| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let (tx, rx) = mpsc::channel::<f64>();
    if let Some(out) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
                    if tx.send(us / 1e6).is_err() {
                        return;
                    }
                }
            }
        });
    }
    let kill = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(t) => {
                if expected > 0.0 {
                    on_frac((t / expected).clamp(0.0, 1.0));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            kill(&mut child);
            return Err(CANCELLED.into());
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("ffmpeg failed: {e}")),
        }
    };
    let stderr = err_reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stderr).into_owned())
    } else {
        let tail = util::last_lines(&stderr, 2);
        Err(if tail.is_empty() { format!("ffmpeg stopped with exit code {}", status.code().unwrap_or(-1)) } else { tail })
    }
}

/// Deletes the partial result unless disarmed.
struct Partial(Option<PathBuf>);
impl Drop for Partial {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// The keyframe of `file`'s picture nearest to each planned start (within
/// `within` seconds), or the planned time when none is found. Relative to the file's start.
pub fn measure_starts(ffprobe: &Path, file: &Path, planned: &[f64], within: f64) -> Vec<f64> {
    let start = ffprobe_out(ffprobe, &["-v", "error", "-show_entries", "format=start_time", "-of", "default=nw=1:nk=1"], file).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
    planned
        .iter()
        .map(|&t| {
            let iv = format!("{}%{}", f6(t - 2.0), f6(t + 2.0));
            let keys: Vec<f64> = ffprobe_out(ffprobe, &["-v", "error", "-select_streams", "v:0", "-read_intervals", &iv, "-show_entries", "packet=pts_time,flags", "-of", "csv=p=0"], file)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let (p, f) = l.split_once(',')?;
                    f.contains('K').then(|| p.trim().parse::<f64>().ok()).flatten()
                })
                .collect();
            let best = keys.into_iter().min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).filter(|k| (k - t).abs() <= within);
            best.unwrap_or(t) - start
        })
        .collect()
}

/// A finished join, at its final path.
#[derive(Debug)]
pub struct Done {
    pub file: PathBuf,
    pub mode: Mode,
    /// Where each clip starts in the result (the first is about 0), measured.
    pub starts: Vec<f64>,
    pub plan: Plan,
    pub duration: f64,
}

/// Join `srcs` (in order) into `out` (which must not exist yet; written as
/// `.<name>.joining` first). `encoder` as in the settings ("auto", "nvenc", ...).
/// `progress` gets 0..100, never backwards. Nothing is left behind on error or cancel.
pub fn join(ffmpeg: &Path, srcs: &[Src], fps_hint: &[u32], out: &Path, title: &str, encoder: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<Done, String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
    let mut plan = plan(srcs, fps_hint)?;
    let dir = out.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "joined.mp4".into());
    let part = dir.join(format!(".{name}.joining"));
    let mut guard = Partial(Some(part.clone()));
    let list = util::tmp_dir().join(format!("join-{}.txt", util::new_id()));
    let _list_guard = Partial(Some(list.clone()));
    let mut top = 0f32;
    let mut report = |p: f32| {
        if p > top {
            top = p;
            progress(p);
        }
    };
    util::log(&format!(
        "join: {} clips -> {}: mode {}, drift {:.1} ms, {:.1} s, tracks {:?}",
        srcs.len(), out.display(), plan.mode.name(), plan.drift * 1000.0, plan.total, plan.tracks
    ));
    let ran = |plan: &Plan, report: &mut dyn FnMut(f32)| -> Result<String, String> {
        let _ = fs::remove_file(&part);
        match plan.mode {
            Mode::Copy | Mode::CopyVideo => {
                fs::write(&list, concat_list(srcs, &plan.durs)).map_err(|e| format!("Could not write the join list: {e}"))?;
                run_ffmpeg(ffmpeg, &copy_args(srcs, plan, &list, title, &part), plan.total, cancel, &mut |f| report(1.0 + 95.0 * f as f32))
            }
            Mode::Encode => {
                let enc = crate::editor::pick_encoder(ffmpeg, encoder);
                let r = run_ffmpeg(ffmpeg, &encode_args(srcs, plan, &enc, title, &part), plan.total, cancel, &mut |f| report(1.0 + 95.0 * f as f32));
                match r {
                    Err(e) if e != CANCELLED && enc != "x264" => {
                        util::log(&format!("join: {enc} failed ({e}), retrying with libx264"));
                        let _ = fs::remove_file(&part);
                        run_ffmpeg(ffmpeg, &encode_args(srcs, plan, "x264", title, &part), plan.total, cancel, &mut |f| report(1.0 + 95.0 * f as f32))
                    }
                    r => r,
                }
            }
        }
    };
    let mut warnings = ran(&plan, &mut report)?;
    // A straight copy whose timestamps still came out of order: lay the sound out again instead.
    let disordered = |w: &str| {
        let w = w.to_ascii_lowercase();
        w.contains("non-monotonic") || w.contains("non monotonically") || w.contains("non-monotonous")
    };
    if plan.mode == Mode::Copy && disordered(&warnings) {
        util::log(&format!("join: copy gave out-of-order timestamps ({}), copying the picture only", warnings.lines().next().unwrap_or("")));
        plan.mode = Mode::CopyVideo;
        let p2 = self::plan_copy_video(srcs, &plan);
        plan = p2;
        warnings = ran(&plan, &mut report)?;
    }
    if !warnings.trim().is_empty() {
        util::log(&format!("join: ffmpeg said: {}", util::last_lines(warnings.as_bytes(), 3)));
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    if !part.is_file() {
        return Err("ffmpeg wrote no file".into());
    }
    // Measured: the keyframe each clip starts with (every clip starts on one; "encode" forces them).
    let within = if plan.mode == Mode::Encode { 0.1 } else { 0.25 };
    let starts = measure_starts(&ffprobe, &part, &plan.starts, within);
    let duration = library::probe(ffmpeg, &part, false).map(|p| p.duration).unwrap_or(0.0);
    if fs::rename(&part, out).is_err() {
        return Err("Could not save the joined clip".into());
    }
    guard.0 = None;
    report(100.0);
    Ok(Done { file: out.to_path_buf(), mode: plan.mode, starts, plan, duration })
}

/// How far ffmpeg moves a result read through the concat demuxer so that no
/// timestamp is negative: the first file's earliest packet (AAC priming, about
/// -21 ms) becomes 0, even when only the picture is taken from it.
fn lift(s0: &Src) -> f64 {
    let lowest = s0.a.iter().map(|a| a.first).fold(s0.video().first, f64::min) - s0.start;
    (-lowest).max(0.0)
}

/// The "copy-video" plan: copy the picture, each clip's frames right after the
/// last one's end; the sound is laid out again (also used when a straight copy misbehaves).
fn plan_copy_video(srcs: &[Src], p: &Plan) -> Plan {
    let n = srcs.len();
    let mut durs = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let (s, nx) = (&srcs[i], &srcs[i + 1]);
        durs.push((s.video().end - s.start) - (nx.video().first - nx.start) + GAP);
    }
    let lift = lift(&srcs[0]);
    let mut starts = Vec::with_capacity(n);
    let mut at = 0.0;
    for (i, s) in srcs.iter().enumerate() {
        starts.push(at + s.video().first - s.start + lift);
        if i < n - 1 {
            at += durs[i];
        }
    }
    let total = starts[n - 1] + p.lens[n - 1];
    Plan { mode: Mode::CopyVideo, durs, starts, total, ..p.clone() }
}

// ---------------------------------------------------------------------------
// The job

/// "<first title> + <n> more", short enough for a file name.
pub fn joined_title(first: &str, more: usize) -> String {
    let head: String = first.trim().chars().take(48).collect();
    format!("{} + {more} more", head.trim_end())
}

/// Clips `ids` from the library, oldest first; refuses what "Edit together" cannot take.
pub fn pick(core: &Core, ids: &[String]) -> Result<Vec<ClipMeta>, String> {
    let mut uniq: Vec<&String> = Vec::new();
    for id in ids {
        if !uniq.contains(&id) {
            uniq.push(id);
        }
    }
    if uniq.len() < 2 {
        return Err("Pick at least two clips to edit together".into());
    }
    if uniq.len() > MAX_CLIPS {
        return Err(format!("Edit together takes at most {MAX_CLIPS} clips"));
    }
    let lib = core.lib.lock().unwrap();
    let mut clips: Vec<ClipMeta> = Vec::with_capacity(uniq.len());
    for id in uniq {
        clips.push(lib.get(id).ok_or("Clip not found")?);
    }
    drop(lib);
    let total: f64 = clips.iter().map(|c| c.duration.max(0.0)).sum();
    if total > MAX_TOTAL_SECS {
        return Err(format!("Edit together takes at most {} minutes of clips", (MAX_TOTAL_SECS / 60.0).round()));
    }
    clips.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.title.cmp(&b.title)));
    for c in &clips {
        if !Path::new(&c.path).is_file() {
            return Err(format!("The video file of \"{}\" is missing", c.title));
        }
    }
    Ok(clips)
}

/// Queue "Edit together" of `ids`. The job's `clip_id` is "join:<ids, oldest first>",
/// so the same selection cannot be queued twice; its title is the new clip's.
pub fn enqueue(core: &Arc<Core>, ids: Vec<String>) -> Result<Job, String> {
    let clips = pick(core, &ids)?;
    let key = format!("join:{}", clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(","));
    if core.jobs.list().iter().any(|j| j.clip_id == key && matches!(j.state.as_str(), "queued" | "running")) {
        return Err("Those clips are already being joined".into());
    }
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    let title = joined_title(&clips[0].title, clips.len() - 1);
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let job_title = title.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let started = Instant::now();
        let core = weak.upgrade().ok_or("Glint is closing")?;
        for c in &clips {
            if !Path::new(&c.path).is_file() {
                return Err(format!("The video file of \"{}\" is missing", c.title));
            }
        }
        let ffprobe = library::ffprobe_for(&ffmpeg).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
        let mut srcs = Vec::with_capacity(clips.len());
        for c in &clips {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELLED.into());
            }
            srcs.push(probe(&ffprobe, Path::new(&c.path), &c.title)?);
        }
        progress(1.0);
        let first = &clips[0];
        let first_path = PathBuf::from(&first.path);
        let dir = first_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&s.clips_dir));
        let out = util::unique_path(&dir, &util::sanitize_filename(&title), "mp4");
        let fps: Vec<u32> = clips.iter().map(|c| c.fps).collect();
        let done = join(&ffmpeg, &srcs, &fps, &out, &title, &s.encoder, cancel, &mut |p| progress(p))?;
        let mut new = library::register(&ffmpeg, &done.file, &title, &first.game, "joined");
        new.folder = first.folder.clone();
        new.joins = clean_joins(&done.starts, new.duration.max(done.duration));
        if new.fps == 0 || done.mode != Mode::Encode {
            new.fps = clips.iter().map(|c| c.fps).max().unwrap_or(new.fps).max(new.fps);
        }
        core.lib.lock().unwrap().add(new.clone());
        core.emit("clips-changed", new.id.clone());
        let secs = started.elapsed().as_secs_f64();
        util::log(&format!("join: done in {secs:.1} s, mode {}, joins {:?}", done.mode.name(), new.joins));
        core.emit("clip-joined", event_payload(&new, done.mode, secs));
        Ok(new.id)
    });
    core.jobs.enqueue(&key, &job_title, "join", run)
}

/// The cut points to store: every start but the first, rounded to the millisecond,
/// in order and inside the clip.
pub fn clean_joins(starts: &[f64], duration: f64) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for &t in starts.iter().skip(1) {
        let t = (t * 1000.0).round() / 1000.0;
        if t > 0.1 && (duration <= 0.0 || t < duration - 0.1) && out.last().is_none_or(|&l| t > l + 0.1) {
            out.push(t);
        }
    }
    out
}

/// The "clip-joined" payload.
fn event_payload(clip: &ClipMeta, mode: Mode, secs: f64) -> Value {
    json!({ "clip": clip, "joins": clip.joins, "mode": mode.name(), "secs": secs })
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    path: PathBuf,
    failed: u32,
    passed: u32,
}

impl Report {
    fn line(&mut self, text: &str) {
        use std::io::Write;
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{text}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.passed += 1;
        } else {
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

fn build(ffmpeg: &Path, args: &[String]) -> Result<(), String> {
    let out = util::hidden_cmd(ffmpeg).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err(util::last_lines(&out.stderr, 2)) }
}

/// One synthetic clip: a black picture with a one-frame white flash at `flash`
/// seconds, and on every audio track a 1 kHz beep starting at the same moment.
struct Spec<'a> {
    w: u32,
    h: u32,
    fps: u32,
    secs: f64,
    /// Audio length minus picture length, seconds.
    audio_extra: f64,
    tracks: u32,
    vfr: bool,
    sar: &'a str,
    flash: f64,
}

fn make_clip(ffmpeg: &Path, out: &Path, sp: &Spec) -> Result<(), String> {
    let frame = 1.0 / sp.fps as f64;
    let mut vf = format!("color=c=black:s={}x{}:r={}:d={},drawbox=c=white:t=fill:enable='between(t,{},{})',setsar={}", sp.w, sp.h, sp.fps, sp.secs, sp.flash - frame * 0.25, sp.flash + frame * 0.6, sp.sar);
    if sp.vfr {
        // Uneven frame times (still increasing): about +-3 ms around the grid.
        vf.push_str(&format!(",settb=1/90000,setpts='(N/{}+0.003*sin(N*1.7))/TB'", sp.fps));
    }
    let asecs = sp.secs + sp.audio_extra;
    let mut a: Vec<String> = sv(&["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"]);
    a.push(vf);
    for k in 0..sp.tracks {
        let f = 1000 + 300 * k;
        a.extend(sv(&["-f", "lavfi", "-i"]));
        a.push(format!("aevalsrc='if(between(t,{},{}),0.8*sin(2*PI*{f}*t),0)|if(between(t,{},{}),0.8*sin(2*PI*{f}*t),0)':s=48000:d={asecs}", sp.flash, sp.flash + 0.06, sp.flash, sp.flash + 0.06));
    }
    a.extend(sv(&["-map", "0:v"]));
    for k in 0..sp.tracks {
        a.push("-map".into());
        a.push(format!("{}:a", k + 1));
    }
    a.extend(sv(&["-c:v", "libx264", "-preset", "ultrafast", "-bf", "0", "-level", "4.0", "-pix_fmt", "yuv420p", "-g"]));
    a.push(sp.fps.to_string());
    // Recorded clips keep the capture's 90 kHz clock.
    a.extend(sv(&["-video_track_timescale", "90000"]));
    if sp.vfr {
        a.extend(sv(&["-fps_mode", "passthrough"]));
    }
    if sp.tracks > 0 {
        a.extend(sv(&["-c:a", "aac", "-b:a", "128k", "-ac", "2"]));
        let names = ["Mix", "Desktop", "Microphone"];
        for k in 0..sp.tracks as usize {
            a.push(format!("-metadata:s:a:{k}"));
            a.push(format!("title={}", names.get(k).copied().unwrap_or("Extra")));
        }
    }
    a.push(out.to_string_lossy().into_owned());
    build(ffmpeg, &a)
}

/// Times of the white flashes (picture timestamps, relative to the file's start).
fn flashes(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    let Ok(out) = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(file)
        .args(["-map", "0:v:0", "-vf", "scale=16:16,format=gray,showinfo", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stderr);
    let mut found = Vec::new();
    let mut prev = 0u32;
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

/// Starts of the beeps on track `k`, decoding the audio back to back (as a player does).
/// The time is counted from the track's first decoded sample plus the track's start time.
fn beeps(ffmpeg: &Path, ffprobe: &Path, file: &Path, k: usize) -> Vec<f64> {
    let Ok(out) = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-map", &format!("0:a:{k}"), "-ac", "1", "-ar", "48000", "-f", "s16le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let start = ffprobe_out(ffprobe, &["-v", "error", "-select_streams", &format!("a:{k}"), "-show_entries", "stream=start_time", "-of", "default=nw=1:nk=1"], file).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
    let mut found = Vec::new();
    let mut quiet = 48_000usize;
    for (i, b) in out.stdout.chunks_exact(2).enumerate() {
        let s = i16::from_le_bytes([b[0], b[1]]).unsigned_abs();
        if s > 3000 {
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

/// Audio minus picture offset of each flash that has a beep within 0.3 s.
fn offsets(fl: &[f64], be: &[f64]) -> Vec<Option<f64>> {
    fl.iter().map(|&f| be.iter().map(|&b| b - f).filter(|d| d.abs() < 0.3).min_by(|a, b| a.abs().total_cmp(&b.abs()))).collect()
}

fn fingerprint(file: &Path) -> (u64, Option<std::time::SystemTime>, u64) {
    let m = fs::metadata(file).ok();
    let data = fs::read(file).unwrap_or_default();
    // FNV-1a over the whole (small) file.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    (m.as_ref().map(|m| m.len()).unwrap_or(0), m.and_then(|m| m.modified().ok()), h)
}

fn stream_titles(ffprobe: &Path, file: &Path) -> Vec<String> {
    let Some(text_json) = ffprobe_out(ffprobe, &["-v", "error", "-select_streams", "a", "-show_entries", "stream_tags=title,name", "-of", "json"], file) else { return Vec::new() };
    let j: Value = serde_json::from_str(&text_json).unwrap_or(Value::Null);
    j["streams"].as_array().cloned().unwrap_or_default().iter().map(|s| [text(&s["tags"]["title"]), text(&s["tags"]["name"])].into_iter().find(|x| !x.is_empty()).unwrap_or_default()).collect()
}

fn wait_until(secs: f64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// `glint.exe --join-test <report.txt>`: headless checks of "Edit together".
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0, passed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run_self_test(&mut r);
    let (passed, failed) = (r.passed, r.failed);
    r.line(&format!("DONE passed={passed} failed={failed}"));
}

struct Case<'a> {
    name: &'a str,
    files: Vec<(&'a str, Spec<'a>)>,
    mode: Mode,
    tracks: usize,
    titles: Vec<&'a str>,
    size: Option<(u32, u32, u32)>,
}

fn run_self_test(r: &mut Report) {
    // 1. Pure parts.
    r.check("title: \"<first> + 2 more\"", joined_title("Valorant 2026-11-12 20-01-02", 2) == "Valorant 2026-11-12 20-01-02 + 2 more", &joined_title("Valorant 2026-11-12 20-01-02", 2));
    r.check("joins kept in order, inside the clip, rounded", clean_joins(&[0.02, 3.00049, 3.05, 7.5, 9.95], 10.0) == vec![3.0, 7.5], &format!("{:?}", clean_joins(&[0.02, 3.00049, 3.05, 7.5, 9.95], 10.0)));
    let list = concat_list(&[Src { path: PathBuf::from(r"C:\a b\it's.mp4"), ..Default::default() }, Src { path: PathBuf::from(r"C:\x.mp4"), ..Default::default() }], &[3.5]);
    r.check("concat list quotes a ' in a path and sets the duration", list.contains(r"file 'C:\a b\it'\''s.mp4'") && list.contains("duration 3.500000") && list.matches("duration").count() == 1, &list.replace('\n', " / "));

    // Plans on made-up probes: 4 s of picture, AAC of 189 packets (4.032 s played, priming -21.3 ms).
    let mk = |tb: &str, frames: u64, w: u32| Src {
        title: "x".into(),
        duration: 4.0,
        v: Some(VInfo { codec: "h264".into(), width: w, height: 720, sar: "1:1".into(), time_base: tb.into(), fps: 60.0, first: 0.0, end: 4.0, ..Default::default() }),
        a: vec![AInfo { codec: "aac".into(), rate: 48000, channels: 2, time_base: "1/48000".into(), first: -1024.0 / 48000.0, frames, title: "Mix".into(), ..Default::default() }],
        ..Default::default()
    };
    let p = plan(&[mk("1/90000", 189, 1280), mk("1/90000", 189, 1280)], &[]);
    r.check("plan: same clips are copied, no drift, the second starts where the first's sound ends", p.as_ref().is_ok_and(|p| p.mode == Mode::Copy && p.drift.abs() <= GAP + 1e-9 && (p.starts[1] - (189.0 * 1024.0 / 48000.0 + GAP + 1024.0 / 48000.0)).abs() < 1e-6), &format!("{:?}", p.as_ref().map(|p| (p.mode, p.drift, p.starts.clone()))));
    let p = plan(&[mk("1/90000", 180, 1280), mk("1/90000", 180, 1280), mk("1/90000", 180, 1280)], &[]);
    r.check("plan: sound 160 ms shorter than the picture drifts, so only the picture is copied", p.as_ref().is_ok_and(|p| p.mode == Mode::CopyVideo && p.drift < -MAX_DRIFT), &format!("{:?}", p.as_ref().map(|p| (p.mode, p.drift))));
    let p = plan(&[mk("1/90000", 189, 1280), mk("1/15360", 189, 1280)], &[60, 60]);
    r.check("plan: a different picture time base is re-encoded (the concat demuxer would scramble it)", p.as_ref().is_ok_and(|p| p.mode == Mode::Encode), &format!("{:?}", p.as_ref().map(|p| p.mode)));
    let p = plan(&[mk("1/90000", 189, 1280), mk("1/90000", 189, 1920)], &[60, 120]);
    r.check("plan: different sizes: encode at the largest size and highest rate", p.as_ref().is_ok_and(|p| p.mode == Mode::Encode && (p.width, p.height, p.fps) == (1920, 720, 120)), &format!("{:?}", p.as_ref().map(|p| (p.mode, p.width, p.height, p.fps))));
    let mut silent = mk("1/90000", 189, 1280);
    silent.a.clear();
    let p = plan(&[mk("1/90000", 189, 1280), silent], &[]);
    r.check("plan: a clip without audio: picture copied, one mixed track", p.as_ref().is_ok_and(|p| p.mode == Mode::CopyVideo && p.tracks == Tracks::Mix), &format!("{:?}", p.as_ref().map(|p| (p.mode, p.tracks.clone()))));
    let mut blind = mk("1/90000", 189, 1280);
    blind.v = None;
    blind.title = "Audio only".into();
    let p = plan(&[mk("1/90000", 189, 1280), blind], &[]);
    r.check("plan: a clip without a picture is refused by name", p.as_ref().err().is_some_and(|e| e == "\"Audio only\" has no picture"), &format!("{:?}", p.err()));

    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let Some(ffprobe) = library::ffprobe_for(&ffmpeg) else {
        r.check("ffprobe found", false, "");
        return;
    };
    let root = util::data_dir().join("clips");
    let _ = fs::remove_dir_all(&root);
    let dir = root.join("Test Game");
    let _ = fs::create_dir_all(&dir);

    let sp = |w, h, fps, secs, audio_extra, tracks, vfr, sar, flash| Spec { w, h, fps, secs, audio_extra, tracks, vfr, sar, flash };
    let cases = vec![
        Case {
            name: "same format (one VFR, 3 tracks)",
            files: vec![("a1", sp(320, 240, 60, 4.0, -0.012, 3, false, "1", 1.0)), ("a2", sp(320, 240, 60, 5.0, 0.02, 3, true, "1", 2.5))],
            mode: Mode::Copy,
            tracks: 3,
            titles: vec!["Mix", "Desktop", "Microphone"],
            size: None,
        },
        Case {
            name: "same format, sound 90 ms short in each (would drift)",
            files: vec![("d1", sp(320, 240, 60, 3.0, -0.09, 1, false, "1", 1.0)), ("d2", sp(320, 240, 60, 3.0, -0.09, 1, false, "1", 1.5)), ("d3", sp(320, 240, 60, 3.0, -0.09, 1, false, "1", 2.0))],
            mode: Mode::CopyVideo,
            tracks: 1,
            titles: vec!["Mix"],
            size: None,
        },
        Case {
            name: "one clip without audio",
            files: vec![("s1", sp(320, 240, 60, 3.0, 0.0, 0, false, "1", 1.0)), ("s2", sp(320, 240, 60, 3.0, 0.0, 2, false, "1", 1.2))],
            mode: Mode::CopyVideo,
            tracks: 1,
            titles: vec!["Mix"],
            size: None,
        },
        Case {
            name: "different sizes, rates and track counts",
            files: vec![
                ("e1", sp(320, 240, 60, 3.0, 0.0, 3, false, "1", 1.0)),
                ("e2", sp(240, 240, 30, 3.0, 0.0, 1, false, "4/3", 1.5)),
                ("e3", sp(160, 200, 30, 2.5, 0.0, 2, true, "1", 0.8)),
            ],
            mode: Mode::Encode,
            tracks: 1,
            titles: vec!["Mix"],
            size: Some((320, 240, 60)),
        },
    ];

    let mut encode_srcs: Vec<Src> = Vec::new();
    for case in &cases {
        let mut paths = Vec::new();
        let mut built = true;
        for (name, spec) in &case.files {
            let p = dir.join(format!("{name}.mp4"));
            if let Err(e) = make_clip(&ffmpeg, &p, spec) {
                r.check(&format!("{}: build {name}", case.name), false, &e);
                built = false;
            }
            paths.push(p);
        }
        if !built {
            continue;
        }
        let before: Vec<_> = paths.iter().map(|p| fingerprint(p)).collect();
        let srcs: Vec<Src> = paths.iter().zip(&case.files).filter_map(|(p, (n, _))| probe(&ffprobe, p, n).ok()).collect();
        if srcs.len() != paths.len() {
            r.check(&format!("{}: probe the clips", case.name), false, "");
            continue;
        }
        // Each source's own flash and its audio/video offset (the VFR clip's flash sits a few ms off its beep).
        let src_flash: Vec<f64> = paths.iter().map(|p| flashes(&ffmpeg, p).first().copied().unwrap_or(-1.0)).collect();
        let src_off: Vec<Option<f64>> = paths
            .iter()
            .zip(&case.files)
            .map(|(p, (_, s))| if s.tracks > 0 { offsets(&flashes(&ffmpeg, p), &beeps(&ffmpeg, &ffprobe, p, 0)).first().copied().flatten() } else { None })
            .collect();
        let out = dir.join(format!("{} joined.mp4", case.files[0].0));
        let t0 = Instant::now();
        let mut pcts = Vec::new();
        let res = join(&ffmpeg, &srcs, &[], &out, "test join", "x264", &AtomicBool::new(false), &mut |p| pcts.push(p));
        let took = t0.elapsed().as_secs_f64();
        let Ok(done) = res else {
            r.check(&format!("{}: join succeeds", case.name), false, res.err().as_deref().unwrap_or(""));
            continue;
        };
        r.check(&format!("{}: join succeeds", case.name), true, "");
        r.line(&format!("INFO {}: mode {}, planned drift of a copy {:.1} ms, {took:.2} s, starts {:?} (planned {:?})", case.name, done.mode.name(), done.plan.drift * 1000.0, done.starts, done.plan.starts));
        r.check(&format!("{}: way taken is {}", case.name, case.mode.name()), done.mode == case.mode, done.mode.name());
        let sum: f64 = srcs.iter().map(|s| s.duration).sum();
        let p = library::probe(&ffmpeg, &out, false).unwrap_or_default();
        r.check(&format!("{}: length = sum of the clips within 50 ms", case.name), (p.duration - sum).abs() <= 0.05, &format!("{:.3} vs {:.3}", p.duration, sum));
        r.check(&format!("{}: one video and {} audio streams", case.name, case.tracks), p.video_streams == 1 && p.audio_streams as usize == case.tracks, &format!("{} video, {} audio", p.video_streams, p.audio_streams));
        let titles = stream_titles(&ffprobe, &out);
        r.check(&format!("{}: track titles {:?}", case.name, case.titles), titles == case.titles, &format!("{titles:?}"));
        let errs = decode_errors(&ffmpeg, &out);
        r.check(&format!("{}: result decodes without errors", case.name), errs.is_empty(), &errs);
        if let Some((w, h, fps)) = case.size {
            r.check(&format!("{}: picture {w}x{h} at {fps} fps", case.name), p.width == w && p.height == h && p.fps == fps, &format!("{}x{}@{}", p.width, p.height, p.fps));
        }
        // Join points: each clip's flash lands at its start + its own flash time.
        let fl = flashes(&ffmpeg, &out);
        let first_starts = srcs.iter().map(|s| s.video().first - s.start).collect::<Vec<_>>();
        let mut worst = 0.0f64;
        let mut ok = fl.len() == srcs.len() && done.starts.len() == srcs.len();
        if ok {
            for i in 0..srcs.len() {
                let want = done.starts[i] + (src_flash[i] - first_starts[i]);
                worst = worst.max((fl[i] - want).abs());
            }
            ok = worst <= 0.05;
        }
        r.check(&format!("{}: every clip starts where the join point says (within 50 ms)", case.name), ok, &format!("worst {:.1} ms, flashes {fl:?}", worst * 1000.0));
        // Sync: at every clip with sound, the beep is as far from the flash as in the clip itself.
        if case.tracks > 0 {
            for k in 0..case.tracks {
                let off = offsets(&fl, &beeps(&ffmpeg, &ffprobe, &out, k));
                let mut worst = 0.0f64;
                let mut good = off.len() == srcs.len();
                for (i, (_, spec)) in case.files.iter().enumerate() {
                    if !good {
                        break;
                    }
                    match (spec.tracks > 0, off.get(i).copied().flatten(), src_off[i]) {
                        (true, Some(o), Some(so)) => worst = worst.max((o - so).abs()),
                        (false, None, _) => {}
                        _ => good = false,
                    }
                }
                r.check(&format!("{}: track {} in sync at every join (within 40 ms; silent clips silent)", case.name, k + 1), good && worst <= 0.04, &format!("worst {:.1} ms, offsets {off:?}, sources {src_off:?}", worst * 1000.0));
            }
        }
        r.check(&format!("{}: source files untouched", case.name), paths.iter().map(|p| fingerprint(p)).collect::<Vec<_>>() == before, "");
        r.check(&format!("{}: progress reached 100 and never went back", case.name), pcts.last() == Some(&100.0) && pcts.windows(2).all(|w| w[1] >= w[0]), &format!("{} updates", pcts.len()));
        let left: Vec<_> = fs::read_dir(&dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".joining")).collect()).unwrap_or_default();
        r.check(&format!("{}: no partial file left", case.name), left.is_empty(), &format!("{left:?}"));
        let _ = fs::remove_file(&out);
        if case.mode == Mode::Encode {
            encode_srcs = srcs;
        }
    }

    // 3. Cancel half-way through an encode.
    if !encode_srcs.is_empty() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let out = dir.join("cancelled.mp4");
        let t0 = Instant::now();
        let mut set_at: Option<Instant> = None;
        let res = join(&ffmpeg, &encode_srcs, &[], &out, "cancel", "x264", &cancel, &mut |p| {
            if p > 20.0 && !c.swap(true, Ordering::SeqCst) {
                set_at = Some(Instant::now());
            }
        });
        let _ = t0;
        r.check("cancel returns \"Cancelled\"", res.as_ref().err().map(String::as_str) == Some(CANCELLED), &format!("{:?}", res.as_ref().map(|d| d.file.clone())));
        r.check("cancel stops within a second", set_at.is_some_and(|t| t.elapsed() < Duration::from_secs(1)), "");
        let left: Vec<_> = fs::read_dir(&dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("cancelled")).collect()).unwrap_or_default();
        r.check("cancel removes the partial file", left.is_empty() && !out.exists(), &format!("{left:?}"));
        let lists: Vec<_> = fs::read_dir(util::tmp_dir()).map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("join-")).collect()).unwrap_or_default();
        r.check("no join lists left in the temp folder", lists.is_empty(), &format!("{lists:?}"));
    }

    // 4. Through the queue and the library.
    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = root.to_string_lossy().into_owned();
        s.encoder = "x264".into();
    }
    core.jobs.connect(&core);
    let mut ids = Vec::new();
    // Registered newest first, so the join must sort them back.
    for (n, (name, created)) in [("a2", 2_000u64), ("a1", 1_000u64)].iter().enumerate() {
        let p = dir.join(format!("{name}.mp4"));
        let mut m = library::register(&ffmpeg, &p, &format!("Test {name}"), "Test Game", "clip");
        m.created = *created;
        m.folder = "Test Game".into();
        m.fps = 60;
        let _ = n;
        ids.push(m.id.clone());
        core.lib.lock().unwrap().add(m);
    }
    r.check("one clip is refused", enqueue(&core, vec![ids[0].clone()]).is_err_and(|e| e == "Pick at least two clips to edit together"), "");
    r.check("the same clip twice counts once (refused)", enqueue(&core, vec![ids[0].clone(), ids[0].clone()]).is_err(), "");
    let many: Vec<String> = (0..21).map(|i| format!("x{i}")).collect();
    r.check("21 clips are refused", enqueue(&core, many).is_err_and(|e| e == "Edit together takes at most 20 clips"), "");
    {
        let mut lib = core.lib.lock().unwrap();
        for (id, dur) in [("long1", 1000.0), ("long2", 900.0)] {
            lib.add(ClipMeta { id: id.into(), path: dir.join("a1.mp4").to_string_lossy().into_owned(), title: id.into(), duration: dur, ..Default::default() });
        }
        lib.add(ClipMeta { id: "gone".into(), path: dir.join("gone.mp4").to_string_lossy().into_owned(), title: "Gone clip".into(), duration: 5.0, created: 5, ..Default::default() });
    }
    r.check("over 30 minutes is refused", enqueue(&core, vec!["long1".into(), "long2".into()]).is_err_and(|e| e == "Edit together takes at most 30 minutes of clips"), "");
    let gone = enqueue(&core, vec![ids[0].clone(), "gone".into()]);
    r.check("a missing file is refused, naming the clip", gone.as_ref().is_err_and(|e| e == "The video file of \"Gone clip\" is missing"), &format!("{:?}", gone.err()));
    r.check("nothing was queued by the refusals", core.jobs.list().is_empty(), "");

    let job = enqueue(&core, ids.clone());
    r.check("join queued as a job of kind \"join\"", job.as_ref().is_ok_and(|j| j.kind == "join" && j.title == "Test a1 + 1 more"), &format!("{:?}", job.as_ref().map(|j| (&j.kind, &j.title)).map_err(|e| e.clone())));
    let dup = enqueue(&core, vec![ids[1].clone(), ids[0].clone()]);
    r.check("the same selection twice is refused while it runs", dup.as_ref().is_err_and(|e| e == "Those clips are already being joined"), &format!("{:?}", dup.err()));
    if let Ok(job) = job {
        let state = |jid: &str| core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();
        let ended = wait_until(120.0, || !matches!(state(&job.id).state.as_str(), "queued" | "running"));
        let j = state(&job.id);
        r.check("the join job finishes", ended && j.state == "done", &format!("{} {}", j.state, j.message));
        match core.lib.lock().unwrap().get(&j.result_id) {
            Some(n) => {
                let p = Path::new(&n.path);
                r.check("library: kind \"joined\"", n.kind == "joined", &n.kind);
                r.check("library: title is the oldest clip's + 1 more", n.title == "Test a1 + 1 more", &n.title);
                r.check("library: file beside the first clip, named after the title", p.parent() == Some(dir.as_path()) && p.file_name().is_some_and(|f| f == "Test a1 + 1 more.mp4"), &n.path);
                r.check("library: game and folder of the first clip", n.game == "Test Game" && n.folder == "Test Game", &format!("{} / {}", n.game, n.folder));
                r.check("library: one join point, where the second clip starts (about 4.0 s)", n.joins.len() == 1 && (n.joins[0] - 4.0).abs() < 0.06, &format!("{:?}", n.joins));
                r.check("library: thumbnail made", !n.thumb.is_empty() && Path::new(&n.thumb).is_file(), &n.thumb);
                r.check("library: length about 9 s, 3 tracks", (n.duration - 9.0).abs() < 0.1 && n.audio_tracks == 3, &format!("{:.3} s, {} tracks", n.duration, n.audio_tracks));
                let saved = fs::read_to_string(util::data_dir().join("library.json")).unwrap_or_default();
                r.check("library.json keeps the join points", saved.contains("\"joins\":["), "");
                let ev = event_payload(&n, Mode::Copy, 1.5);
                r.check("clip-joined payload has clip, joins, mode, secs", ev["clip"]["id"] == n.id.as_str() && ev["joins"].as_array().is_some_and(|a| a.len() == 1) && ev["mode"] == "copy" && ev["secs"] == 1.5, &ev.to_string());
            }
            None => r.check("result registered in the library", false, &j.result_id),
        }
    }
    let plain = core.lib.lock().unwrap().get(&ids[0]).map(|c| serde_json::to_string(&c).unwrap_or_default()).unwrap_or_default();
    r.check("a plain clip's entry has no \"joins\" key", !plain.is_empty() && !plain.contains("joins"), "");
    core.jobs.shutdown();

    // 5. Optional: GLINT_JOIN_FILES="<a.mp4>|<b.mp4>|..." joins real clips (read only) into the test folder.
    if let Some(list) = std::env::var_os("GLINT_JOIN_FILES") {
        let paths: Vec<PathBuf> = list.to_string_lossy().split('|').filter(|s| !s.trim().is_empty()).map(|s| PathBuf::from(s.trim())).collect();
        let srcs: Vec<Src> = paths.iter().filter_map(|p| probe(&ffprobe, p, &p.to_string_lossy()).ok()).collect();
        let out = dir.join("real join.mp4");
        let t0 = Instant::now();
        match join(&ffmpeg, &srcs, &[], &out, "real join", "auto", &AtomicBool::new(false), &mut |_| {}) {
            Ok(d) => {
                let sum: f64 = srcs.iter().map(|s| s.duration).sum();
                r.line(&format!("INFO real clips: {} files, mode {}, drift of a copy {:.1} ms, {:.1} s, length {:.3} vs sum {sum:.3}, starts {:?} (planned {:?})", srcs.len(), d.mode.name(), d.plan.drift * 1000.0, t0.elapsed().as_secs_f64(), d.duration, d.starts, d.plan.starts));
                r.check("real clips: the result decodes without errors", decode_errors(&ffmpeg, &out).is_empty(), &decode_errors(&ffmpeg, &out));
                r.check("real clips: length = sum within 50 ms (+ AAC priming per join)", (d.duration - sum).abs() <= 0.05 + 0.045 * (srcs.len() - 1) as f64, &format!("{:.3} vs {sum:.3}", d.duration));
                let _ = fs::remove_file(&out);
            }
            Err(e) => r.check("real clips: join", false, &e),
        }
    }
}

/// What ffmpeg complains about while decoding the whole file ("" = nothing). Frames keep
/// their own times (a recorded clip is variable frame rate; forcing a constant rate on the
/// null output makes ffmpeg complain about its own rounding, for the source clips too).
fn decode_errors(ffmpeg: &Path, file: &Path) -> String {
    util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(file)
        .args(["-fps_mode", "passthrough", "-enc_time_base:v", "demux", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map(|o| util::last_lines(&o.stderr, 3))
        .unwrap_or_else(|e| e.to_string())
}
