//! Export of a project that references its library clips (0.1.38, "virtual" projects: no joined
//! working file). The options are what the editor sends (`emToOpts`), in PROJECT TIME: the
//! project's clips laid end to end in the order they were added, clip k from `start_k` for
//! `duration_k` seconds.
//!
//! - Every kept range in ONE source: that clip's own file is exported directly (`editor::export`;
//!   one trimmed piece is then still the lossless cut).
//! - Ranges in several sources: the sources they use are joined, in project order, into a temporary
//!   working file with the join a draft used (`join::join`, matching), every time is moved onto that
//!   file's measured joins, `editor::export` runs on it, and the file is removed whatever happens
//!   (a guard that also runs while a panic unwinds).
//!
//! Times in the options that are PROJECT time, and how they are mapped (`s` in clip k):
//! - `segments[].start / end` (and `start` / `end` without segments): `s - start_k` in that clip's
//!   file, or `real_start_j + (s - start_k)` in the joined file (j = the clip's place among the
//!   clips used). A range that crosses from one clip into the next is split at the border.
//! - `audio[].regions` of track rows and of file rows cut with the video: the same, split at clip
//!   borders; the parts on clips that are not exported are dropped.
//! - `audio[].offset` of a file row cut with the video: the row is split at clip borders into one
//!   row per clip it covers (`offset` mapped, `trim_start` moved on by the part before, `length`
//!   the part's length).
//! - `audio[].index` and `tracks[].index` (track rows): an index into the PROJECT's track titles
//!   (`join::Tracks::of` over every present source, the titles a draft of all the clips would have)
//!   is moved to the exported file's track with that title; a row whose title that file lacks is left
//!   out (it would be silent there anyway).
//! - Everything else is output time (overlays, blurs, fades, the canvas tracks, `tail`, rows that
//!   play through cuts) and stays as it is.
use crate::editor::{self, AudioEdit, ExportOpts, GainRegion, Range, TrackVolume};
use crate::join;
use crate::library::{self, ClipMeta};
use crate::settings::Settings;
use crate::util;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// One clip of a project, in project order.
#[derive(Clone, Debug, Default)]
pub struct Source {
    /// The library clip's id ("" if unknown).
    pub clip: String,
    pub title: String,
    pub path: String,
    /// Its length in the library: the stretch of project time it covers.
    pub duration: f64,
    /// Where it starts in project time (the sum of the lengths before it).
    pub start: f64,
    pub fps: u32,
    /// The library no longer has the clip, or its file is gone.
    pub missing: bool,
}

/// The project being exported: the result is named and filed as a draft's export was.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub id: String,
    pub title: String,
    pub game: String,
    pub folder: String,
}

/// How many temporary working files were joined (tests).
pub static JOINS_MADE: AtomicU32 = AtomicU32::new(0);

/// The name every temporary working file starts with, in `util::tmp_dir()`.
pub const TMP_PREFIX: &str = "projexport-";

/// Removes the temporary working file when dropped, whatever happened.
struct TmpFile(PathBuf);

impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        // join.rs writes `.<name>.joining` first and removes it itself; this is for a panic in between.
        if let (Some(dir), Some(name)) = (self.0.parent(), self.0.file_name()) {
            let _ = std::fs::remove_file(dir.join(format!(".{}.joining", name.to_string_lossy())));
        }
    }
}

/// A stretch of project time in clip `k`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Piece {
    k: usize,
    a: f64,
    b: f64,
}

/// The parts of `a..b` (project time) in each clip it covers, in order.
fn pieces(sources: &[Source], a: f64, b: f64) -> Vec<Piece> {
    let mut out = Vec::new();
    for (k, s) in sources.iter().enumerate() {
        let (lo, hi) = (a.max(s.start), b.min(s.start + s.duration));
        if hi - lo > 1e-4 {
            out.push(Piece { k, a: lo, b: hi });
        }
    }
    out
}

/// The clips the kept ranges of `opts` use (indices into `sources`), and the ranges split at clip borders.
fn used(sources: &[Source], opts: &ExportOpts) -> (Vec<usize>, Vec<Range>) {
    let total: f64 = sources.last().map(|s| s.start + s.duration).unwrap_or(0.0);
    let given: Vec<Range> = if opts.segments.is_empty() {
        let start = if opts.start.is_finite() { opts.start.clamp(0.0, total) } else { 0.0 };
        let end = if opts.end.is_finite() && opts.end > start { opts.end.min(total) } else { total };
        vec![Range { start, end, color: Some(opts.color.clone()), ..Default::default() }]
    } else {
        opts.segments.clone()
    };
    let mut segs: Vec<Range> = Vec::new();
    let mut ks: Vec<usize> = Vec::new();
    for g in given {
        if g.gap.is_finite() && g.gap > 0.0005 {
            segs.push(g);
            continue;
        }
        for p in pieces(sources, g.start, g.end) {
            if !ks.contains(&p.k) {
                ks.push(p.k);
            }
            segs.push(Range { start: p.a, end: p.b, ..g.clone() });
        }
    }
    ks.sort_unstable();
    (ks, segs)
}

/// Project time -> time in the exported file, for the clips in `exported` (their file starts).
struct Map<'a> {
    sources: &'a [Source],
    /// (clip index, where it starts in the exported file).
    exported: Vec<(usize, f64)>,
}

impl Map<'_> {
    fn at(&self, k: usize, s: f64) -> Option<f64> {
        self.exported.iter().find(|(j, _)| *j == k).map(|(_, f)| f + (s - self.sources[k].start))
    }
    /// Where clip `k`'s footage ends in the exported file: the next clip's start in a joined file (a clip's
    /// library length can be longer than its picture: R3-6, a range to its end showed the next clip's first frames).
    fn end_of(&self, k: usize) -> f64 {
        let i = self.exported.iter().position(|(j, _)| *j == k);
        i.and_then(|i| self.exported.get(i + 1)).map(|(_, f)| f - 0.0005).unwrap_or(f64::INFINITY)
    }
    /// `a..b` in the exported file: the parts on exported clips.
    fn span(&self, a: f64, b: f64) -> Vec<(f64, f64, Piece)> {
        pieces(self.sources, a, b).into_iter().filter_map(|p| Some((self.at(p.k, p.a)?, self.at(p.k, p.b)?, p))).collect()
    }
}

/// Where the project's track row `i` is heard in the exported file: `rows[i]` = the file's audio streams
/// that carry it, each with the stretches of that file (seconds) where it must be silent because there
/// it holds a clip that has nothing for that row. Empty = nothing for it anywhere.
type RowMap = Vec<Vec<(u32, Vec<(f64, f64)>)>>;

/// `opts` (project time) as options for the exported file (see `RowMap`).
fn map_opts(opts: &ExportOpts, segs: Vec<Range>, map: &Map, rows: &RowMap) -> ExportOpts {
    let mut o = opts.clone();
    o.track_titles = Vec::new();
    o.start = 0.0;
    o.end = 0.0;
    o.segments = segs
        .into_iter()
        .filter_map(|g| {
            if g.gap.is_finite() && g.gap > 0.0005 {
                return Some(g);
            }
            let p = pieces(map.sources, g.start, g.end).into_iter().next()?;
            let end = map.at(p.k, p.b)?.min(map.end_of(p.k));
            Some(Range { start: map.at(p.k, p.a)?, end, ..g })
        })
        .collect();
    // Moved parts: the order is the list's; in project order the mapped ranges keep theirs.
    let track_to_file = |i: u32| -> Option<u32> { rows.get(i as usize).and_then(|v| v.first()).map(|x| x.0) };
    let regions = |rs: &[GainRegion]| -> Vec<GainRegion> { rs.iter().flat_map(|g| map.span(g.start, g.end).into_iter().map(move |(a, b, _)| GainRegion { start: a, end: b, gain: g.gain })).collect() };
    let mut audio: Vec<AudioEdit> = Vec::new();
    for e in &opts.audio {
        match e.source.as_str() {
            "track" => {
                // One row per stream that carries it; silent (gain 0, listed first so it wins) where that
                // stream holds a clip without the row's title.
                for (index, zeros) in rows.get(e.index as usize).cloned().unwrap_or_default() {
                    let mut rs: Vec<GainRegion> = zeros.iter().map(|&(a, b)| GainRegion { start: a, end: b, gain: 0.0 }).collect();
                    rs.extend(regions(&e.regions));
                    audio.push(AudioEdit { index, regions: rs, ..e.clone() });
                }
            }
            "file" if e.through => audio.push(e.clone()), // on the result's own time
            "file" => {
                // Cut with the video: one row per clip it covers.
                let len = if e.length > 0.0 { e.length } else { f64::INFINITY };
                let total = map.sources.last().map(|s| s.start + s.duration).unwrap_or(0.0);
                for (a, _b, p) in map.span(e.offset, (e.offset + len).min(total.max(e.offset))) {
                    let rs: Vec<GainRegion> = e.regions.iter().flat_map(|g| map.span(g.start.max(p.a), g.end.min(p.b)).into_iter().map(move |(x, y, _)| GainRegion { start: x, end: y, gain: g.gain })).collect();
                    audio.push(AudioEdit { offset: a, trim_start: e.trim_start + (p.a - e.offset), length: p.b - p.a, regions: rs, ..e.clone() });
                }
            }
            _ => audio.push(e.clone()),
        }
    }
    // A mix that had rows and lost them all would mean "keep the clip's tracks": say "nothing heard" instead.
    if audio.is_empty() && !opts.audio.is_empty() {
        o.tracks = vec![TrackVolume { index: 0, volume: 0.0 }];
    }
    o.audio = audio;
    if !opts.tracks.is_empty() {
        o.tracks = opts.tracks.iter().filter_map(|t| Some(TrackVolume { index: if t.volume <= 0.0 { t.index } else { track_to_file(t.index)? }, volume: t.volume })).collect();
    }
    o
}

/// The track titles of each source (as `join` names them) and the project's titles over all present sources.
fn track_titles(ffmpeg: &Path, files: &[&Source]) -> Result<(Vec<join::Src>, join::Tracks), String> {
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("Could not read the clips (ffprobe.exe was not found)")?;
    let list: Vec<(&Path, &str)> = files.iter().map(|s| (Path::new(&s.path), s.title.as_str())).collect();
    let srcs = join::probe_all(&ffprobe, &list).into_iter().collect::<Result<Vec<_>, _>>()?;
    let tracks = join::Tracks::of(&srcs);
    Ok((srcs, tracks))
}

/// Export project `target` made of `sources` with `opts` in project time. `progress`: 0..100
/// (the join first, then the encode).
/// `export` that `cancel_export { id }` ends, the join included (R3-7).
pub fn export_cancellable(s: &Settings, target: &Target, sources: &[Source], opts: &ExportOpts, id: &str, progress: &mut dyn FnMut(&str, f32)) -> Result<ClipMeta, String> {
    let guard = editor::ExportGuard::new(id);
    let flag = guard.flag.clone();
    editor::with_cancel(&guard.flag, || export_with(s, target, sources, opts, &flag, progress))
}

/// `export` without a cancel (tests).
pub fn export(s: &Settings, target: &Target, sources: &[Source], opts: &ExportOpts, progress: &mut dyn FnMut(&str, f32)) -> Result<ClipMeta, String> {
    export_with(s, target, sources, opts, &AtomicBool::new(false), progress)
}

fn export_with(s: &Settings, target: &Target, sources: &[Source], opts: &ExportOpts, cancel: &AtomicBool, progress: &mut dyn FnMut(&str, f32)) -> Result<ClipMeta, String> {
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    if sources.is_empty() {
        return Err("The project has no clips".into());
    }
    let (ks, segs) = used(sources, opts);
    if ks.is_empty() {
        return Err("The selection is too short".into());
    }
    // A clip the edit needs is gone: say which, before anything is made.
    for &k in &ks {
        let src = &sources[k];
        if src.missing || !Path::new(&src.path).is_file() {
            util::log(&format!("export project {}: clip {} (\"{}\") is missing", target.id, src.clip, src.title));
            return Err(format!("The clip \"{}\" of this project is missing (deleted, or its file was moved), so the project cannot be exported", src.title));
        }
    }
    // 0.1.39: an audio clip that is a source's own sound (an ungrouped piece) names that source's file:
    // when that file is gone it is the same error as a missing source, before anything is made.
    for e in opts.audio.iter().filter(|e| e.source == "file") {
        if Path::new(&e.path).is_file() {
            continue;
        }
        let same = |p: &str| !p.is_empty() && Path::new(p).to_string_lossy().to_lowercase() == Path::new(&e.path).to_string_lossy().to_lowercase();
        if let Some(src) = sources.iter().find(|x| same(&x.path)) {
            util::log(&format!("export project {}: the sound of clip {} (\"{}\") is missing", target.id, src.clip, src.title));
            return Err(format!("The clip \"{}\" of this project is missing (deleted, or its file was moved), so the project cannot be exported", src.title));
        }
    }    // The project's track titles: over every source that is there (what a draft of them all had).
    let present: Vec<&Source> = sources.iter().filter(|x| !x.missing && Path::new(&x.path).is_file()).collect();
    let (_, project) = track_titles(&ffmpeg, &present)?;
    let meta_for = |path: &Path, fps: u32| -> ClipMeta {
        let p = library::probe(&ffmpeg, path, false).unwrap_or_default();
        ClipMeta {
            id: target.id.clone(),
            path: path.to_string_lossy().into_owned(),
            title: target.title.clone(),
            game: target.game.clone(),
            created: util::unix_ms(),
            duration: p.duration,
            width: p.width,
            height: p.height,
            fps: if fps > 0 { fps } else { p.fps },
            kind: "joined".into(),
            audio_tracks: p.audio_streams,
            folder: target.folder.clone(),
            draft: true,
            ..Default::default()
        }
    };

    if ks.len() == 1 {
        // One clip: its own file, directly (no join, no temporary file).
        let k = ks[0];
        let src = &sources[k];
        // A project track is the stream of this clip a draft would have put on it (join's rules: a clip
        // with only a Mix is heard on Desktop too).
        let at = present.iter().position(|x| std::ptr::eq(*x, src)).unwrap_or(0);
        let rows: RowMap = if opts.track_titles.is_empty() {
            (0..project.titles.len()).map(|t| project.feeds.get(at).and_then(|f| f.get(t)).copied().flatten().map(|x| vec![(x as u32, Vec::new())]).unwrap_or_default()).collect()
        } else {
            // 0.1.38: rows by title; this clip's own shown track with that title, else silent.
            let shown = shown_tracks(&ffmpeg, Path::new(&src.path));
            opts.track_titles.iter().map(|t| shown.iter().find(|(_, n)| n == t).map(|(i, _)| vec![(*i as u32, Vec::new())]).unwrap_or_default()).collect()
        };
        let map = Map { sources, exported: vec![(k, 0.0)] };
        let mapped = map_opts(opts, segs, &map, &rows);
        let meta = meta_for(Path::new(&src.path), src.fps);
        util::log(&format!("export project {}: one clip, \"{}\", exported from its own file", target.id, src.title));
        let mut mapped = mapped;
        fit_to_project(&ffmpeg, &mut mapped, sources, Path::new(&src.path));
        return editor::export(s, &meta, &mapped, &mut |p| progress("export", p));
    }

    // Several clips: the ones used, joined into a temporary working file.
    let chosen: Vec<&Source> = ks.iter().map(|&k| &sources[k]).collect();
    let (srcs, _) = track_titles(&ffmpeg, &chosen)?;
    let fps: Vec<u32> = chosen.iter().map(|x| x.fps).collect();
    let tmp = util::tmp_dir();
    let _ = std::fs::create_dir_all(&tmp);
    let file = tmp.join(format!("{TMP_PREFIX}{}-{}.mp4", util::sanitize_filename(&target.id), util::new_id()));
    let _guard = TmpFile(file.clone());
    JOINS_MADE.fetch_add(1, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let done = join::join(&ffmpeg, &srcs, &fps, false, &file, &target.title, &s.encoder, cancel, &mut |p| progress("join", p))
        .map_err(|e| if e == join::CANCELLED || cancel.load(Ordering::SeqCst) { editor::CANCELLED.to_string() } else { format!("Could not put the project's clips together for the export: {e}") })?;
    util::log(&format!("export project {}: {} clips joined in {} ms ({}), starts {:?}", target.id, chosen.len(), started.elapsed().as_millis(), done.mode.name(), done.starts));
    // The joined file has the tracks join gave it, by title.
    let file_titles = done.plan.tracks.titles.clone();
    let rows: RowMap = if opts.track_titles.is_empty() {
        project.titles.iter().map(|t| file_titles.iter().position(|x| x == t).map(|x| vec![(x as u32, Vec::new())]).unwrap_or_default()).collect()
    } else {
        // 0.1.38: rows by title across the clips. Clip j is heard on row T from its own shown track
        // titled T, which the join put on some track of the file; a row becomes one row per such file
        // track, silent where that track holds a clip with nothing (or something else) for T.
        let feeds = &done.plan.tracks.feeds;
        let spans: Vec<(f64, f64)> = (0..chosen.len()).map(|j| (done.starts[j], done.starts.get(j + 1).copied().unwrap_or(f64::INFINITY))).collect();
        let shown: Vec<Vec<(usize, String)>> = chosen.iter().map(|x| shown_tracks(&ffmpeg, Path::new(&x.path))).collect();
        opts.track_titles
            .iter()
            .map(|t| {
                let mut groups: Vec<(u32, Vec<usize>)> = Vec::new();
                for (j, sh) in shown.iter().enumerate() {
                    let Some(&(stream, _)) = sh.iter().find(|(_, n)| n == t) else { continue };
                    let names = join::track_names(&srcs[j].a);
                    let on = |jt: &usize| feeds.get(j).and_then(|f| f.get(*jt)).copied().flatten() == Some(stream);
                    // The file track that carries that stream, the one of its own name first.
                    let jt = (0..file_titles.len()).filter(on).find(|jt| names.get(stream) == file_titles.get(*jt)).or_else(|| (0..file_titles.len()).find(on));
                    if let Some(jt) = jt {
                        match groups.iter_mut().find(|g| g.0 == jt as u32) {
                            Some(g) => g.1.push(j),
                            None => groups.push((jt as u32, vec![j])),
                        }
                    }
                }
                groups.into_iter().map(|(jt, js)| (jt, (0..chosen.len()).filter(|j| !js.contains(j)).map(|j| spans[j]).collect())).collect()
            })
            .collect()
    };
    let map = Map { sources, exported: ks.iter().copied().zip(done.starts.iter().copied()).collect() };
    let mapped = map_opts(opts, segs, &map, &rows);
    let fps_out = fps.iter().copied().max().unwrap_or(0);
    let meta = meta_for(&file, fps_out);
    let mut mapped = mapped;
    fit_to_project(&ffmpeg, &mut mapped, sources, &file);
    editor::export(s, &meta, &mapped, &mut |p| progress("export", p))
}

/// R2-13: a project is drawn at its own picture size (the first clip's): when the exported file has another size
/// and the edit has no canvas, the file is fitted inside a canvas of the project's size, as the preview shows it,
/// so overlays drawn at the project's size land where they were put.
fn fit_to_project(ffmpeg: &Path, opts: &mut ExportOpts, sources: &[Source], file: &Path) {
    if opts.canvas.is_some() {
        return;
    }
    let Some(first) = sources.iter().find(|x| !x.missing && Path::new(&x.path).is_file()) else { return };
    let size = |p: &Path| library::probe(ffmpeg, p, false).map(|q| (q.width as f64, q.height as f64)).filter(|(w, h)| *w > 0.0 && *h > 0.0);
    let (Some((pw, ph)), Some((fw, fh))) = (size(Path::new(&first.path)), size(file)) else { return };
    if (pw - fw).abs() < 0.5 && (ph - fh).abs() < 0.5 {
        return;
    }
    let (pa, fa) = (pw / ph, fw / fh);
    let (w, h) = if fa > pa { (1.0, pa / fa) } else { (fa / pa, 1.0) };
    let piece = editor::CanvasPiece { sx: 0.0, sy: 0.0, sw: 1.0, sh: 1.0, x: (1.0 - w) / 2.0, y: (1.0 - h) / 2.0, w, h };
    opts.canvas = Some(editor::Canvas { width: pw, height: ph, background: "black".into(), pieces: vec![piece] });
    util::log(&format!("export project: the file is {fw}x{fh}, the project {pw}x{ph}: fitted inside"));
}

/// A file's audio tracks as the editor shows them (`emShownTracks`): (stream index, title), the title
/// being the stream's title or "Track <n>"; without "Mix" when there are titled tracks, without track 0
/// for an untitled 3-track file.
fn shown_tracks(ffmpeg: &Path, file: &Path) -> Vec<(usize, String)> {
    let titles = crate::clip::audio_titles(ffmpeg, file);
    let all: Vec<(usize, String)> = titles.iter().enumerate().map(|(i, t)| (i, if t.is_empty() { format!("Track {}", i + 1) } else { t.clone() })).collect();
    if all.len() > 1 && titles.iter().any(|t| t == "Mix") {
        all.into_iter().filter(|(_, t)| t != "Mix").collect()
    } else if all.len() == 3 {
        all.into_iter().filter(|(i, _)| *i != 0).collect()
    } else {
        all
    }
}

/// Startup (R3-8): Glint's own temporary working files that a killed or crashed export left behind, removed by
/// their exact names (`projexport-*`, `.projexport-*`, join's `match-*`) and only when they are older than this
/// run of Glint. Returns how many went.
pub fn sweep_tmp() -> usize {
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs_f64(util::now_secs());
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(util::tmp_dir()) else { return 0 };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = (name.starts_with(TMP_PREFIX) || name.starts_with(&format!(".{TMP_PREFIX}")) || name.starts_with("match-")) && (name.ends_with(".mp4") || name.ends_with(".joining"));
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < started);
        if ours && old && e.path().is_file() && std::fs::remove_file(e.path()).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        util::log(&format!("startup: {n} temporary export files of an earlier run removed"));
    }
    n
}

/// Temporary working files left in the temp folder (tests: none after any export).
pub fn leftovers() -> Vec<PathBuf> {
    std::fs::read_dir(util::tmp_dir())
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TMP_PREFIX) || n.starts_with(&format!(".{TMP_PREFIX}")))).collect())
        .unwrap_or_default()
}

/// The project `id` as this export needs it: its clips in order, found in the library, with their
/// starts on project time (the sum of the library lengths before each). The only part that reads the
/// project store (`projects.rs`, agent P's): a clip the library no longer has, or whose file is gone,
/// is `missing`.
pub fn sources_of(core: &crate::core::Core, id: &str) -> Result<(Target, Vec<Source>), String> {
    let p = crate::projects::get(core, id).ok_or("Project not found")?;
    let lib = core.lib.lock().unwrap();
    let mut start = 0.0;
    let mut out = Vec::new();
    let mut first: Option<ClipMeta> = None;
    for c in &p.clips {
        // F1 (0.1.41): an old project's draft working file is its first source, named by the project's own id.
        let meta = if c.id == p.id { p.clip.clone().filter(|d| Path::new(&d.path).is_file()) } else { lib.get(&c.id) };
        // Project time is laid out from each clip's length WHEN IT WAS ADDED (projects::virtual_clip does the same for
        // the editor), so a saved edit and the export agree even if the library's clip changed length since.
        let duration = if c.duration > 0.0 { c.duration } else { meta.as_ref().map(|m| m.duration).unwrap_or(0.0) };
        let missing = !meta.as_ref().is_some_and(|m| Path::new(&m.path).is_file());
        if first.is_none() {
            first = meta.clone();
        }
        out.push(Source {
            clip: c.id.clone(),
            title: meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| c.title.clone()),
            path: meta.as_ref().map(|m| m.path.clone()).unwrap_or_default(),
            duration,
            start,
            fps: meta.as_ref().map(|m| m.fps).unwrap_or(0),
            missing,
        });
        start += duration;
    }
    let target = Target { id: p.id.clone(), title: p.name.clone(), game: first.as_ref().map(|m| m.game.clone()).unwrap_or_default(), folder: first.as_ref().map(|m| m.folder.clone()).unwrap_or_default() };
    Ok((target, out))
}