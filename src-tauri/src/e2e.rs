//! `glint.exe --e2e-test <report.txt>` (0.1.37): half 2 of the end-to-end test from the editor's
//! real edit model to the real export. Half 1, `node tools/e2e-scenarios.js <folder>`, plays edits
//! through `ui/editmodel.js` and writes `clip.json` (the synthetic clip both halves agree on) and one
//! `sNN.json` per scenario: the options the model sends, and what the result must be. Glint never
//! starts node or any script: `tools/e2e.ps1` runs the two halves one after the other, and this
//! half reads the folder named by `GLINT_E2E_DIR`. Without scenarios it fails (it never passes empty).
//!
//! The clip: a white bar `bar` px wide moves across a black picture; at source second `s` its left
//! edge is at `trunc(s * (width - bar) / duration)`, so the source second on screen in any frame of a
//! result can be read back from where the bar is. Every audio track is a `tone` Hz sine.
use crate::editor::{self, ExportOpts};
use crate::library;
use crate::settings::Settings;
use crate::util;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Deserialize, Debug, Clone)]
struct ClipSpec {
    duration: f64,
    width: u32,
    height: u32,
    fps: u32,
    bar: u32,
    tone: u32,
    tracks: Vec<TrackSpec>,
    song: SongSpec,
    /// 0.1.38: separate clip files for project scenarios.
    #[serde(default)]
    sources: Vec<SourceSpec>,
}

/// A clip file of its own (0.1.38): its bar only in its third of the picture (`band`), its own tone.
#[derive(Deserialize, Debug, Clone)]
struct SourceSpec {
    name: String,
    duration: f64,
    width: u32,
    height: u32,
    fps: u32,
    bar: u32,
    band: u32,
    tone: u32,
    tracks: Vec<TrackSpec>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct ProjectSpec {
    sources: Vec<String>,
    missing: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct TrackSpec {
    title: String,
    /// The track is the `tone` (else silent).
    tone: bool,
    /// 0.1.39: this track's own tone instead of the clip's `tone`.
    #[serde(default)]
    freq: Option<u32>,
}

#[derive(Deserialize, Debug, Clone)]
struct SongSpec {
    file: String,
    freq: u32,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Scenario {
    name: String,
    checks: Vec<ModelCheck>,
    opts: Option<ExportOpts>,
    expect: Option<Expect>,
    model: String,
    project: Option<ProjectSpec>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct ModelCheck {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Expect {
    duration: f64,
    kind: String,
    frames: Vec<Frame>,
    tones: Vec<Tone>,
    /// Projects: the export must fail with this in its message.
    error: String,
    /// Projects: a temporary working file was joined (true) or the one clip's file exported directly.
    joined: Option<bool>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Frame {
    t: f64,
    src: Option<f64>,
    black: bool,
    /// Projects: the source (`SourceSpec::name`) on screen.
    source: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
struct Tone {
    freq: u32,
    from: f64,
    to: f64,
    heard: bool,
    /// 0.1.39: heard this many dB from the source's level (a row at half level: -6.02); None = as recorded.
    db: Option<f64>,
}

struct Report {
    path: PathBuf,
    passed: u32,
    failed: u32,
}

impl Report {
    fn line(&self, text: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{text}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok { self.passed += 1 } else { self.failed += 1 }
        let tag = if ok { "PASS" } else { "FAIL" };
        self.line(&if detail.is_empty() { format!("{tag} {name}") } else { format!("{tag} {name} - {detail}") });
    }
}

/// How far (source seconds) the second read from a frame may be from the one expected: about two
/// frames, and the bar's own pixel step.
const SRC_TOL: f64 = 0.12;

pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), passed: 0, failed: 0 };
    let _ = std::fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run(&mut r);
    r.line(&format!("DONE passed={} failed={}", r.passed, r.failed));
}

fn run(r: &mut Report) {
    let Some(dir) = std::env::var_os("GLINT_E2E_DIR").map(PathBuf::from).filter(|d| d.is_dir()) else {
        r.check("scenarios", false, "no GLINT_E2E_DIR folder: run `node tools/e2e-scenarios.js <folder>` first (tools/e2e.ps1 does both)");
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    files.retain(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('s') && n.ends_with(".json") && n[1..n.len() - 5].chars().all(|c| c.is_ascii_digit())));
    files.sort();
    let spec: Option<ClipSpec> = std::fs::read_to_string(dir.join("clip.json")).ok().and_then(|t| serde_json::from_str(&t).ok());
    let Some(spec) = spec.filter(|_| !files.is_empty()) else {
        r.check("scenarios", false, &format!("{} has no clip.json or no scenario files: run `node tools/e2e-scenarios.js` first", dir.display()));
        return;
    };
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let work = util::data_dir().join("e2e");
    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::create_dir_all(&work);
    let Some(src) = make_clip(&ffmpeg, &spec, &work) else {
        r.check("the synthetic clip is made", false, "ffmpeg could not write it");
        return;
    };
    // The clip as both halves described it.
    let probe = library::probe(&ffmpeg, Path::new(&src.path), true);
    let as_said = probe.as_ref().is_some_and(|p| (p.duration - spec.duration).abs() < 0.05 && p.width == spec.width && p.height == spec.height && p.audio_streams as usize == spec.tracks.len());
    r.check("the synthetic clip is the one clip.json describes", as_said, &format!("{:?}", probe.as_ref().map(|p| (p.duration, p.width, p.height, p.audio_streams))));
    let bar_ok = [1.0, spec.duration / 2.0, spec.duration - 1.0].iter().all(|&s| read_second(&ffmpeg, Path::new(&src.path), s + 0.5 / spec.fps as f64, &spec).is_some_and(|x| (x - s).abs() < SRC_TOL));
    r.check("the source second can be read back from a frame of the clip", bar_ok, "");
    let ref_tone = band_db(&ffmpeg, Path::new(&src.path), 0, spec.tone, 0.5, spec.duration - 0.5);
    let ref_song = band_db(&ffmpeg, &dir.join(&spec.song.file), 0, spec.song.freq, 0.5, 4.0);
    r.line(&format!("INFO {} scenarios in {}; tone {ref_tone:?} dB, song {ref_song:?} dB", files.len(), dir.display()));
    let s = Settings { clips_dir: work.to_string_lossy().into_owned(), game_folders: false, ..Default::default() };

    for f in &files {
        let file = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sc: Scenario = match std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
            Ok(v) => v,
            Err(e) => {
                r.check(&format!("{file}: readable"), false, &e);
                continue;
            }
        };
        let label = format!("{file} {}", sc.name);
        for c in &sc.checks {
            r.check(&format!("{label}: model: {}", c.name), c.ok, &c.detail);
        }
        if let Some(p) = &sc.project {
            let (Some(opts), Some(want)) = (&sc.opts, &sc.expect) else {
                r.check(&format!("{label}: has options and an expectation"), false, "the model could not make this edit (see above)");
                continue;
            };
            project_scenario(r, &ffmpeg, &s, &spec, &dir, &work, &label, p, opts, want, ref_song);
            continue;
        }
        let (Some(mut opts), Some(want)) = (sc.opts, sc.expect) else {
            r.check(&format!("{label}: has options and an expectation"), false, "the model could not make this edit (see above)");
            continue;
        };
        // 0.1.39: an audio clip of the clip's own sound names its file: the node half writes "<clip>".
        for e in &mut opts.audio {
            if e.path == "<clip>" {
                e.path = src.path.clone();
            }
        }
        let started = std::time::Instant::now();
        let before = std::fs::read_dir(&work).map(|d| d.count()).unwrap_or(0);
        let res = editor::export(&s, &src, &opts, &mut |_| {});
        if !want.error.is_empty() {
            let ok = res.as_ref().err().is_some_and(|e| e.contains(&want.error)) && std::fs::read_dir(&work).map(|d| d.count()).unwrap_or(0) == before;
            r.check(&format!("{label}: fails with \"{}\", nothing written", want.error), ok, &format!("{:?}", res.as_ref().map(|m| m.path.clone())));
            continue;
        }
        let m = match res {
            Ok(m) => m,
            Err(e) => {
                r.check(&format!("{label}: exports"), false, &e);
                continue;
            }
        };
        let out = PathBuf::from(&m.path);
        r.line(&format!("INFO {label}: {} ({}) in {} ms, model {}", out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), m.kind, started.elapsed().as_millis(), sc.model));
        if !want.kind.is_empty() {
            r.check(&format!("{label}: kind {}", want.kind), m.kind == want.kind, &m.kind);
        }
        // Length: the container and every stream (a stream's last frame may end one frame early).
        let durs = stream_durations(&ffmpeg, &out);
        let slack = 0.05 + 1.0 / spec.fps as f64;
        let len_ok = !durs.is_empty() && durs.iter().all(|d| (d - want.duration).abs() <= slack) && probe_len(&ffmpeg, &out).is_some_and(|d| (d - want.duration).abs() <= 0.05);
        r.check(&format!("{label}: {:.3} s long", want.duration), len_ok, &format!("streams {durs:?}, file {:?}", probe_len(&ffmpeg, &out)));
        // Frames: the source second on screen, or black.
        let mut bad: Vec<String> = Vec::new();
        for fr in &want.frames {
            let got = read_frame(&ffmpeg, &out, fr.t, &spec);
            match (fr.black, fr.src, &got) {
                (true, _, Some(Seen::Black)) => {}
                (false, Some(s0), Some(Seen::Second(x))) if (*x - s0).abs() <= SRC_TOL => {}
                _ => bad.push(format!("at {:.2} s want {} got {}", fr.t, if fr.black { "black".to_string() } else { format!("{:.2}", fr.src.unwrap_or(f64::NAN)) }, match got { Some(Seen::Black) => "black".into(), Some(Seen::Second(x)) => format!("{x:.2}"), Some(Seen::Other) => "neither".into(), None => "no frame".into() })),
            }
        }
        r.check(&format!("{label}: {} frames show the right source second or black", want.frames.len()), bad.is_empty(), &bad.join("; "));
        // Sound: heard (within 3 dB of the source) or silent (30 dB under it), on the first track.
        let mut bad: Vec<String> = Vec::new();
        for tn in &want.tones {
            // The level of that tone as recorded: the clip's tone, the song, or one track's own tone.
            let reference = if tn.freq == spec.tone {
                ref_tone
            } else if tn.freq == spec.song.freq {
                ref_song
            } else {
                spec.tracks.iter().position(|t| t.tone && t.freq == Some(tn.freq)).and_then(|i| band_db(&ffmpeg, Path::new(&src.path), i as u32, tn.freq, 0.5, spec.duration - 0.5))
            };
            let lv = band_db(&ffmpeg, &out, 0, tn.freq, tn.from, tn.to);
            let ok = match (reference, lv) {
                (Some(r0), Some(v)) => if tn.heard { (v - r0 - tn.db.unwrap_or(0.0)).abs() <= if tn.db.is_some() { 1.5 } else { 3.0 } } else { v < r0 - 30.0 },
                _ => false,
            };
            if !ok {
                bad.push(format!("{} Hz {:.2}..{:.2} s want {} got {lv:?} ({reference:?})", tn.freq, tn.from, tn.to, if tn.heard { "heard" } else { "silent" }));
            }
        }
        r.check(&format!("{label}: {} stretches of sound heard or silent as they should be", want.tones.len()), bad.is_empty(), &bad.join("; "));
        let _ = std::fs::remove_file(&out);
    }
}

/// The synthetic clip of `spec` (three titled tracks, a keyframe each second), registered as a library clip.
fn make_clip(ffmpeg: &Path, spec: &ClipSpec, dir: &Path) -> Option<library::ClipMeta> {
    let out = dir.join("E2E clip.mp4");
    let (w, h, fps, d, bar) = (spec.width, spec.height, spec.fps, spec.duration, spec.bar);
    // The bar is a white picture laid on the black one at x = trunc(t * (width - bar) / duration), frame by frame.
    let video = format!("color=c=black:s={w}x{h}:r={fps}:d={d}[bg];color=c=white:s={bar}x{h}:r={fps}:d={d}[bar];[bg][bar]overlay=x='trunc(t*{}/{d})':y=0:eval=frame:shortest=1", w - bar);
    let mut cmd = util::hidden_cmd(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &video]);
    for t in &spec.tracks {
        let a = if t.tone { format!("sine=frequency={}:sample_rate=48000:duration={d}", t.freq.unwrap_or(spec.tone)) } else { format!("anullsrc=r=48000:cl=stereo,atrim=end={d}") };
        cmd.args(["-f", "lavfi", "-i", &a]);
    }
    cmd.args(["-map", "0:v"]);
    for (i, t) in spec.tracks.iter().enumerate() {
        cmd.args(["-map", &format!("{}:a", i + 1)]).arg(format!("-metadata:s:a:{i}")).arg(format!("title={}", t.title));
    }
    cmd.args(["-c:v", "libx264", "-preset", "ultrafast", "-g", &fps.to_string(), "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-t", &format!("{d}")]).arg(&out);
    let ok = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok || !out.is_file() {
        return None;
    }
    let mut meta = library::register(ffmpeg, &out, "E2E clip", "Desktop", "clip");
    meta.fps = fps;
    Some(meta)
}

enum Seen {
    Black,
    Second(f64),
    Other,
}

/// What the frame at output time `t` shows: black, the bar (the source second it stands for), or neither.
fn read_frame(ffmpeg: &Path, file: &Path, t: f64, spec: &ClipSpec) -> Option<Seen> {
    let w = spec.width as usize;
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", &format!("scale={w}:1:flags=area,format=gray"), "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.stdout.len() != w {
        return None;
    }
    let row = &out.stdout;
    let max = *row.iter().max().unwrap_or(&0);
    if max < 40 {
        return Some(Seen::Black);
    }
    let lit: Vec<usize> = (0..w).filter(|&x| row[x] > 128).collect();
    // The bar and nothing else: a run no wider than the bar and a pixel either side.
    if lit.is_empty() || lit.len() > spec.bar as usize + 2 || lit.last().unwrap() - lit[0] > spec.bar as usize + 1 {
        return Some(Seen::Other);
    }
    let mean = lit.iter().sum::<usize>() as f64 / lit.len() as f64;
    let left = mean - (spec.bar as f64 - 1.0) / 2.0;
    Some(Seen::Second((left * spec.duration / (spec.width - spec.bar) as f64).max(0.0)))
}

fn read_second(ffmpeg: &Path, file: &Path, t: f64, spec: &ClipSpec) -> Option<f64> {
    match read_frame(ffmpeg, file, t, spec)? {
        Seen::Second(x) => Some(x),
        _ => None,
    }
}

/// Mean level (dB) around `freq` Hz of audio stream `stream`, `from..to`, decoded from the start as a player does.
fn band_db(ffmpeg: &Path, file: &Path, stream: u32, freq: u32, from: f64, to: f64) -> Option<f64> {
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
    String::from_utf8_lossy(&out.stderr).lines().find_map(|l| l.split("mean_volume:").nth(1).and_then(|v| v.trim().trim_end_matches("dB").trim().parse::<f64>().ok()))
}

fn ffprobe_lines(ffmpeg: &Path, file: &Path, entries: &str) -> Vec<f64> {
    let Some(ffprobe) = library::ffprobe_for(ffmpeg) else { return Vec::new() };
    util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-show_entries", entries, "-of", "csv=p=0"])
        .arg(file)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse().ok()).collect())
        .unwrap_or_default()
}

fn stream_durations(ffmpeg: &Path, file: &Path) -> Vec<f64> {
    ffprobe_lines(ffmpeg, file, "stream=duration")
}

fn probe_len(ffmpeg: &Path, file: &Path) -> Option<f64> {
    ffprobe_lines(ffmpeg, file, "format=duration").first().copied()
}

/// The files of the project sources, made once per run: `<work>\sources\<name>.mp4`, registered.
fn source_clip(ffmpeg: &Path, sp: &SourceSpec, work: &Path) -> Option<library::ClipMeta> {
    let dir = work.join("sources");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("{}.mp4", sp.name));
    if !out.is_file() {
        let (w, h, fps, d, bar) = (sp.width, sp.height, sp.fps, sp.duration, sp.bar);
        let (bh, y) = (h / 3, sp.band * (h / 3));
        let video = format!("color=c=black:s={w}x{h}:r={fps}:d={d}[bg];color=c=white:s={bar}x{bh}:r={fps}:d={d}[bar];[bg][bar]overlay=x='trunc(t*{}/{d})':y={y}:eval=frame:shortest=1", w - bar);
        let mut cmd = util::hidden_cmd(ffmpeg);
        cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &video]);
        for t in &sp.tracks {
            let a = if t.tone { format!("sine=frequency={}:sample_rate=48000:duration={d}", t.freq.unwrap_or(sp.tone)) } else { format!("anullsrc=r=48000:cl=stereo,atrim=end={d}") };
            cmd.args(["-f", "lavfi", "-i", &a]);
        }
        cmd.args(["-map", "0:v"]);
        for (i, t) in sp.tracks.iter().enumerate() {
            cmd.args(["-map", &format!("{}:a", i + 1)]).arg(format!("-metadata:s:a:{i}")).arg(format!("title={}", t.title));
        }
        cmd.args(["-c:v", "libx264", "-preset", "ultrafast", "-g", &fps.to_string(), "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-t", &format!("{d}")]).arg(&out);
        if !cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
            return None;
        }
    }
    let mut meta = library::register(ffmpeg, &out, &sp.name, "Desktop", "clip");
    meta.fps = sp.fps;
    Some(meta)
}

/// Which source (by its band) and which second of it a frame of a project's result shows, or black.
fn read_project_frame(ffmpeg: &Path, file: &Path, t: f64, sources: &[SourceSpec]) -> Option<(Option<String>, f64)> {
    const W: usize = 640;
    let out = util::hidden_cmd(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{t:.3}"), "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-vf", &format!("scale={W}:3:flags=area,format=gray"), "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if out.stdout.len() != W * 3 {
        return None;
    }
    if out.stdout.iter().all(|v| *v < 40) {
        return Some((None, 0.0));
    }
    for band in 0..3usize {
        let row = &out.stdout[band * W..(band + 1) * W];
        let lit: Vec<usize> = (0..W).filter(|&x| row[x] > 128).collect();
        if lit.is_empty() {
            continue;
        }
        let sp = sources.iter().find(|s| s.band as usize == band)?;
        let centre = (lit.iter().sum::<usize>() as f64 / lit.len() as f64 + 0.5) / W as f64;
        let sec = (centre * sp.width as f64 - sp.bar as f64 / 2.0) * sp.duration / (sp.width - sp.bar) as f64;
        return Some((Some(sp.name.clone()), sec.max(0.0)));
    }
    Some((Some("?".into()), 0.0))
}

#[allow(clippy::too_many_arguments)]
fn project_scenario(r: &mut Report, ffmpeg: &Path, s: &Settings, spec: &ClipSpec, dir: &Path, work: &Path, label: &str, p: &ProjectSpec, opts: &ExportOpts, want: &Expect, ref_song: Option<f64>) {
    use crate::projexport::{self, Source, Target};
    use std::sync::atomic::Ordering;
    // The project's sources, in its order, with their starts on project time.
    let mut sources: Vec<Source> = Vec::new();
    let mut start = 0.0;
    for name in &p.sources {
        let Some(sp) = spec.sources.iter().find(|x| &x.name == name) else {
            r.check(&format!("{label}: source {name} is in clip.json"), false, "");
            return;
        };
        let Some(meta) = source_clip(ffmpeg, sp, work) else {
            r.check(&format!("{label}: source {name} is made"), false, "ffmpeg could not write it");
            return;
        };
        let missing = p.missing.contains(name);
        sources.push(Source { clip: meta.id.clone(), title: meta.title.clone(), path: if missing { work.join("gone.mp4").to_string_lossy().into_owned() } else { meta.path.clone() }, duration: sp.duration, start, fps: sp.fps, missing });
        start += sp.duration;
    }
    // 0.1.39: an audio clip of a source's own sound names its file: the node half writes "<source:NAME>".
    let mut opts = opts.clone();
    for e in &mut opts.audio {
        if let Some(name) = e.path.strip_prefix("<source:").and_then(|x| x.strip_suffix('>')) {
            if let Some(k) = p.sources.iter().position(|n| n == name) {
                e.path = sources[k].path.clone();
            }
        }
    }
    let opts = &opts;
    let target = Target { id: format!("e2e-{}", util::new_id()), title: "E2E project".into(), game: "Desktop".into(), folder: String::new() };
    let results = |w: &Path| std::fs::read_dir(w).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "mp4")).count()).unwrap_or(0);
    let (joins0, files0) = (projexport::JOINS_MADE.load(Ordering::SeqCst), results(work));
    let started = std::time::Instant::now();
    let res = projexport::export(s, &target, &sources, opts, &mut |_| {});
    let joined = projexport::JOINS_MADE.load(Ordering::SeqCst) > joins0;
    let left = projexport::leftovers();
    r.check(&format!("{label}: no temporary working file left"), left.is_empty(), &format!("{left:?}"));
    if let Some(j) = want.joined {
        r.check(&format!("{label}: {}", if j { "the used clips joined into a temporary file" } else { "the one clip's own file, no join" }), joined == j, &format!("joined {joined}"));
    }
    if !want.error.is_empty() {
        let ok = res.as_ref().err().is_some_and(|e| e.contains(&want.error)) && results(work) == files0;
        r.check(&format!("{label}: fails with \"{}\", nothing written", want.error), ok, &format!("{:?}", res.as_ref().map(|m| m.path.clone())));
        return;
    }
    let m = match res {
        Ok(m) => m,
        Err(e) => {
            r.check(&format!("{label}: exports"), false, &e);
            return;
        }
    };
    let out = PathBuf::from(&m.path);
    r.line(&format!("INFO {label}: {} ({}) in {} ms", out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), m.kind, started.elapsed().as_millis()));
    if !want.kind.is_empty() {
        r.check(&format!("{label}: kind {}", want.kind), m.kind == want.kind, &m.kind);
    }
    let durs = stream_durations(ffmpeg, &out);
    let len_ok = !durs.is_empty() && durs.iter().all(|d| (d - want.duration).abs() <= 0.05 + 1.0 / 30.0) && probe_len(ffmpeg, &out).is_some_and(|d| (d - want.duration).abs() <= 0.06);
    r.check(&format!("{label}: {:.3} s long", want.duration), len_ok, &format!("streams {durs:?}"));
    let mut bad: Vec<String> = Vec::new();
    for fr in &want.frames {
        // Bands are told apart within the project's own sources.
        let mine: Vec<SourceSpec> = spec.sources.iter().filter(|x| p.sources.contains(&x.name)).cloned().collect();
        let got = read_project_frame(ffmpeg, &out, fr.t, &mine);
        let ok = match (&got, fr.black) {
            (Some((None, _)), true) => true,
            (Some((Some(name), x)), false) => fr.source.as_ref() == Some(name) && fr.src.is_some_and(|s0| (x - s0).abs() <= SRC_TOL),
            _ => false,
        };
        if !ok {
            bad.push(format!("at {:.2} s want {} got {:?}", fr.t, if fr.black { "black".to_string() } else { format!("{} {:.2}", fr.source.clone().unwrap_or_default(), fr.src.unwrap_or(f64::NAN)) }, got));
        }
    }
    r.check(&format!("{label}: {} frames show the right source and second, or black", want.frames.len()), bad.is_empty(), &bad.join("; "));
    let mut bad: Vec<String> = Vec::new();
    for tn in &want.tones {
        // The source and track that has this tone (a track's own, else the source's on its first track).
        let found = spec.sources.iter().find_map(|x| x.tracks.iter().position(|t| t.tone && t.freq == Some(tn.freq)).map(|i| (x, i as u32))).or_else(|| spec.sources.iter().find(|x| x.tone == tn.freq).map(|x| (x, 0)));
        let reference = match found {
            Some((sp, i)) => source_clip(ffmpeg, sp, work).and_then(|c| band_db(ffmpeg, Path::new(&c.path), i, tn.freq, 0.5, sp.duration - 0.5)),
            None if tn.freq == spec.song.freq => ref_song,
            None => None,
        };
        let lv = band_db(ffmpeg, &out, 0, tn.freq, tn.from, tn.to);
        let ok = match (reference, lv) {
            (Some(r0), Some(v)) => if tn.heard { (v - r0 - tn.db.unwrap_or(0.0)).abs() <= if tn.db.is_some() { 1.5 } else { 3.0 } } else { v < r0 - 30.0 },
            _ => false,
        };
        if !ok {
            bad.push(format!("{} Hz {:.2}..{:.2} s want {} got {lv:?} ({reference:?})", tn.freq, tn.from, tn.to, if tn.heard { "heard" } else { "silent" }));
        }
    }
    r.check(&format!("{label}: {} stretches of sound heard or silent as they should be", want.tones.len()), bad.is_empty(), &bad.join("; "));
    let _ = dir;
    let _ = std::fs::remove_file(&out);
}