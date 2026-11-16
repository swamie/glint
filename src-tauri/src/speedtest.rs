//! `glint.exe --speed-test <report.txt>` (0.1.41, CONTRACT_0141F): how long adding clips to the editor takes,
//! headless, on synthetic clips of a realistic shape (2560x1440, 60 fps, 60 s, 4 and 10 audio tracks, a still
//! picture so they encode quickly at a low bitrate). Every step is timed for 1 clip and for 19, cold (nothing
//! prepared) and warm (asked again), and written as "TIME <step> <ms>" lines; a few PASS / FAIL checks say the
//! steps worked. Needs GLINT_DATA_DIR (the real library and settings are never touched).
use crate::core::Core;
use crate::{editor, import, join, library, projects, util};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

struct Report {
    path: PathBuf,
    lines: Vec<String>,
}

impl Report {
    fn line(&mut self, s: String) {
        self.lines.push(s);
        let _ = fs::write(&self.path, self.lines.join("\n") + "\n");
    }
    fn time(&mut self, step: &str, ms: f64) {
        self.line(format!("TIME {step} {ms:.0}"));
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        self.line(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if ok || detail.is_empty() { String::new() } else { format!(" - {detail}") }));
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), lines: Vec::new() };
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    match std::env::var_os("GLINT_SPEED_REAL") {
        Some(list) => run_real(&mut r, &PathBuf::from(list)),
        None => run(&mut r),
    }
    let fails = r.lines.iter().filter(|l| l.starts_with("FAIL")).count();
    r.line(format!("DONE failed={fails}"));
}

/// GLINT_SPEED_REAL=<a text file of video paths, one per line>: the same steps on real files, imported BY REFERENCE
/// only (nothing is copied, moved or written next to them: the library, thumbnails and caches are in the scratch data
/// folder). Only times and counts are written, never a path or a title.
fn run_real(r: &mut Report, list: &std::path::Path) {
    let paths: Vec<String> = fs::read_to_string(list).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    r.line(format!("INFO real files: {}", paths.len()));
    if paths.is_empty() {
        return r.check("real files listed", false, "");
    }
    let base = util::data_dir().join("speed-real");
    let _ = fs::remove_dir_all(&base);
    let _ = fs::create_dir_all(&base);
    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = base.join("clips").to_string_lossy().into_owned();
        s.game_folders = false;
    }
    core.lib.lock().unwrap().clips.clear();
    core.jobs.connect(&core);
    let n = paths.len();
    let t = Instant::now();
    let res = import::import_files(&core, paths[..1].to_vec(), false, "");
    r.time("REAL import_files ref x1", ms(t));
    let t = Instant::now();
    let res2 = import::import_files(&core, paths[1..].to_vec(), false, "");
    r.time(&format!("REAL import_files ref x{} (one call)", n - 1), ms(t));
    let ids: Vec<String> = res.iter().chain(res2.iter()).flat_map(|v| v.iter().filter_map(|d| d.clip.as_ref().map(|c| c.id.clone()))).collect();
    r.check("REAL: every file is a library clip", ids.len() == n, &format!("{} of {n}", ids.len()));
    let metas: Vec<library::ClipMeta> = {
        let lib = core.lib.lock().unwrap();
        ids.iter().filter_map(|id| lib.get(id)).collect()
    };
    for m in &metas {
        r.line(format!("INFO REAL clip: {:.0} s, {}x{} @ {} fps, {} audio tracks, {} MB", m.duration, m.width, m.height, m.fps, m.audio_tracks, m.size / 1_000_000));
    }
    let p = projects::create(&core, "Speed real");
    let t = Instant::now();
    let a = projects::add(&core, &p.id, ids.clone());
    r.time(&format!("REAL project_add_clips x{n}"), ms(t));
    let t = Instant::now();
    let o = projects::open(&core, &p.id);
    r.time(&format!("REAL project_open ({n} sources)"), ms(t));
    r.check("REAL project add / open", a.is_ok() && o.is_ok(), "");
    let s = core.settings();
    let all = |f: &(dyn Fn(&library::ClipMeta) -> bool + Sync), list: &[library::ClipMeta]| -> (f64, f64, usize) {
        let t = Instant::now();
        let done: Vec<(f64, bool)> = std::thread::scope(|sc| list.iter().map(|m| sc.spawn(move || { let ok = f(m); (ms(t), ok) })).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap_or((0.0, false))).collect());
        (done.iter().map(|d| d.0).fold(f64::INFINITY, f64::min), ms(t), done.iter().filter(|d| d.1).count())
    };
    let tracks = |m: &library::ClipMeta| editor::audio_tracks(&s, m).is_ok();
    let waves = |m: &library::ClipMeta| editor::waveforms(&s, m).is_ok();
    let thumbs = |m: &library::ClipMeta| editor::thumbs(&s, m, 30).is_ok_and(|v| v.len() == 30);
    for (name, f) in [("editor_audio_tracks", &tracks as &(dyn Fn(&library::ClipMeta) -> bool + Sync)), ("editor_waveforms", &waves), ("editor_thumbs", &thumbs)] {
        let (_, t1, ok1) = all(f, &metas[..1]);
        r.time(&format!("REAL {name} x1 cold"), t1);
        let (_, t1w, _) = all(f, &metas[..1]);
        r.time(&format!("REAL {name} x1 warm"), t1w);
        let (first, tn, okn) = all(f, &metas[1..]);
        r.time(&format!("REAL {name} x{} cold (at once) first", n - 1), first);
        r.time(&format!("REAL {name} x{} cold (at once)", n - 1), tn);
        let (_, tnw, _) = all(f, &metas[1..]);
        r.time(&format!("REAL {name} x{} warm (at once)", n - 1), tnw);
        r.check(&format!("REAL {name}: every clip answered"), ok1 == 1 && okn == n - 1, &format!("{ok1} {okn}"));
    }
    core.jobs.shutdown();
    let _ = fs::remove_dir_all(&base);
}

fn run(r: &mut Report) {
    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else { return r.check("ffmpeg found", false, "") };
    let base = util::data_dir().join("speed");
    let _ = fs::remove_dir_all(&base);
    let clips_dir = base.join("clips");
    let src = base.join("src");
    let _ = fs::create_dir_all(&clips_dir);
    let _ = fs::create_dir_all(&src);
    for (name, ok, detail) in crate::edcache::checks(&src) {
        r.check(&name, ok, &detail);
    }

    // Two masters (4 and 10 tracks), copied to as many files as needed (each its own path, so nothing is cached).
    let names10 = ["Mix", "Desktop", "Microphone", "Game", "T5", "T6", "T7", "T8", "T9", "T10"];
    let t = Instant::now();
    let masters: Vec<PathBuf> = [(4u32, &names10[..4]), (10u32, &names10[..])]
        .iter()
        .enumerate()
        .filter_map(|(k, (tracks, names))| {
            let p = src.join(format!("master{k}.mp4"));
            let sp = join::Spec { w: 2560, h: 1440, fps: 60, secs: 60.0, audio_extra: 0.0, tracks: *tracks, vfr: false, sar: "1", flash: 1.0, names };
            join::make_clip(&ffmpeg, &p, &sp).ok().map(|_| p)
        })
        .collect();
    r.check("made the two 1440p60 60 s masters", masters.len() == 2, "");
    if masters.len() != 2 {
        return;
    }
    r.line(format!("INFO masters made in {:.1} s, {} and {} bytes", t.elapsed().as_secs_f64(), fs::metadata(&masters[0]).map(|m| m.len()).unwrap_or(0), fs::metadata(&masters[1]).map(|m| m.len()).unwrap_or(0)));
    let copies = |dir: &str, n: usize| -> Vec<String> {
        let d = src.join(dir);
        let _ = fs::create_dir_all(&d);
        (0..n)
            .map(|i| {
                let p = d.join(format!("Game {i:02} 2026.11.01 - 10.00.{i:02}.00.mp4"));
                let _ = fs::copy(&masters[i % 2], &p);
                p.to_string_lossy().into_owned()
            })
            .collect()
    };

    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = clips_dir.to_string_lossy().into_owned();
        s.game_folders = false;
    }
    core.lib.lock().unwrap().clips.clear();
    core.jobs.connect(&core);

    // --- import_files (a drop): reference and copy, 1 and 19.
    let mut imported: Vec<String> = Vec::new();
    for (step, copy, n) in [("import_files ref x1", false, 1), ("import_files ref x19", false, 19), ("import_files copy x1", true, 1), ("import_files copy x19", true, 19)] {
        let paths = copies(&step.replace(' ', "_"), n);
        let t = Instant::now();
        let res = import::import_files(&core, paths, copy, "");
        r.time(step, ms(t));
        let got: Vec<String> = res.as_ref().map(|v| v.iter().filter_map(|d| d.clip.as_ref().map(|c| c.id.clone())).collect()).unwrap_or_default();
        r.check(&format!("{step}: every file is a library clip"), got.len() == n, &format!("{res:?}"));
        if !copy && n == 19 {
            imported = got;
        }
    }
    // Copies finish in the background since 0.1.41 (F3): wait for them, so they do not run under the steps below.
    let until = Instant::now() + std::time::Duration::from_secs(120);
    while import::copies_running() && Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if imported.len() != 19 {
        return;
    }
    let metas: Vec<library::ClipMeta> = {
        let lib = core.lib.lock().unwrap();
        imported.iter().filter_map(|id| lib.get(id)).collect()
    };

    // --- a project: add 1, add 19 (one call), open.
    let p1 = projects::create(&core, "Speed 1");
    let t = Instant::now();
    let a1 = projects::add(&core, &p1.id, vec![imported[0].clone()]);
    r.time("project_add_clips x1", ms(t));
    let p19 = projects::create(&core, "Speed 19");
    let t = Instant::now();
    let a19 = projects::add(&core, &p19.id, imported.clone());
    r.time("project_add_clips x19", ms(t));
    r.check("project adds answered", a1.is_ok() && a19.is_ok(), "");
    let t = Instant::now();
    let o = projects::open(&core, &p19.id);
    r.time("project_open (19 sources)", ms(t));
    r.check("project_open has 19 sources", o.as_ref().is_ok_and(|v| v["sources"].as_array().map(|s| s.len()) == Some(19)), "");

    // --- an old project with a draft (made before 0.1.38): adding one clip (the owner's case).
    match projects::legacy_add_ms(&core, &imported[1..4], &imported[4..5]) {
        Ok(t) => r.time("old draft project: add 1 clip", t),
        Err(e) => r.check("old draft project: add 1 clip", false, &e),
    }

    // --- the editor's helpers per source: 1 cold / warm, 19 cold / warm (asked all at once, as the editor does).
    let s = core.settings();
    // Asked all at once, as the editor does: (when the first answered, when the last answered, how many worked).
    let all = |f: &(dyn Fn(&library::ClipMeta) -> bool + Sync), list: &[library::ClipMeta]| -> (f64, f64, usize) {
        let t = Instant::now();
        let done: Vec<(f64, bool)> = std::thread::scope(|sc| list.iter().map(|m| sc.spawn(move || { let ok = f(m); (ms(t), ok) })).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap_or((0.0, false))).collect());
        let first = done.iter().map(|d| d.0).fold(f64::INFINITY, f64::min);
        (first, ms(t), done.iter().filter(|d| d.1).count())
    };
    let tracks = |m: &library::ClipMeta| editor::audio_tracks(&s, m).is_ok_and(|v| !v.is_empty());
    let waves = |m: &library::ClipMeta| editor::waveforms(&s, m).is_ok_and(|v| !v.is_empty());
    let thumbs = |m: &library::ClipMeta| editor::thumbs(&s, m, 30).is_ok_and(|v| v.len() == 30);
    for (name, f) in [("editor_audio_tracks", &tracks as &(dyn Fn(&library::ClipMeta) -> bool + Sync)), ("editor_waveforms", &waves), ("editor_thumbs", &thumbs)] {
        let (_, t1, ok1) = all(f, &metas[..1]);
        r.time(&format!("{name} x1 cold"), t1);
        let (_, t1w, _) = all(f, &metas[..1]);
        r.time(&format!("{name} x1 warm"), t1w);
        let (first, t19, ok19) = all(f, &metas[1..]);
        r.time(&format!("{name} x18 cold (at once) first"), first);
        r.time(&format!("{name} x18 cold (at once)"), t19);
        let (_, t19w, _) = all(f, &metas[1..]);
        r.time(&format!("{name} x18 warm (at once)"), t19w);
        r.check(&format!("{name}: every clip answered"), ok1 == 1 && ok19 == 18, &format!("{ok1} {ok19}"));
    }
    core.jobs.shutdown();
    let _ = fs::remove_dir_all(&base);
}
