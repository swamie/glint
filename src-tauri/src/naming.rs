//! Clip names made of blocks (0.1.18).
//!
//! A name is a row of blocks joined by spaces: Game, Date (`2026-11-14`), Time
//! (`13-35-44`), Length (`30s`), Text (the user's own words, any number of
//! them) and, for a bulk rename only, Number and Current name. `build` is the
//! one function that turns blocks into a name; a new clip is named with
//! `clip_title` (setting `clip_name`), and the default blocks give exactly the
//! name Glint has always given: `Overwatch 2026-11-14 13-35-44`.
//!
//! Bulk rename (`plan`, `apply`, `undo`) follows the single rename
//! (`Core::rename_clip`): title and file name are one thing, a clip a job is
//! using is refused, and a clip imported in place (its file outside the clips
//! folder) only changes its title: Glint never renames another program's file.
use crate::core::{in_clips_dir, Core};
use crate::library::ClipMeta;
use crate::util;
use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Longest name a set of blocks can make (the default is at most 60 + 20).
const MAX_NAME: usize = 120;
/// Longest piece of typed text in one Text block.
const MAX_TEXT: usize = 60;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct NameBlock {
    /// "game" | "date" | "time" | "length" | "text" | "number" | "current". Others are ignored.
    pub kind: String,
    /// The words of a "text" block.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl NameBlock {
    pub fn of(kind: &str) -> Self {
        NameBlock { kind: kind.into(), text: String::new() }
    }
    pub fn text(words: &str) -> Self {
        NameBlock { kind: "text".into(), text: words.into() }
    }
}

/// What Glint has always named a clip: Game Date Time.
pub fn default_blocks() -> Vec<NameBlock> {
    ["game", "date", "time"].iter().map(|k| NameBlock::of(k)).collect()
}

/// What fills the blocks for one clip.
pub struct Parts<'a> {
    pub game: &'a str,
    pub when: DateTime<Local>,
    pub secs: f64,
    /// The Number block's text ("007"); None outside a bulk rename.
    pub number: Option<String>,
    /// The clip's present name (Current name block); "" outside a bulk rename.
    pub current: &'a str,
}

/// Text made safe for a file name: no `<>:"/\|?*` or control characters, single
/// spaces, no dots or spaces at the ends, at most `max` characters. "" when nothing is left.
pub fn clean(s: &str, max: usize) -> String {
    let c: String = s.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let c = c.split_whitespace().collect::<Vec<_>>().join(" ");
    let c: String = c.trim_matches(|c| c == '.' || c == ' ').chars().take(max).collect();
    c.trim_end_matches(|c| c == '.' || c == ' ').to_string()
}

/// The Length block: "30s", "1m05s", "10m".
pub fn length_text(secs: f64) -> String {
    let r = if secs.is_finite() && secs > 0.0 { secs.round() as u64 } else { 0 };
    if r < 60 {
        format!("{r}s")
    } else if r % 60 == 0 {
        format!("{}m", r / 60)
    } else {
        format!("{}m{:02}s", r / 60, r % 60)
    }
}

/// The Number block: `n` with at least `pad` digits ("7", "07", "007").
pub fn number_text(n: u32, pad: u32) -> String {
    format!("{:0width$}", n, width = pad.clamp(1, 6) as usize)
}

/// Windows refuses these as file names, with any extension.
fn reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0')
}

fn assemble(blocks: &[NameBlock], p: &Parts) -> String {
    let mut out: Vec<String> = Vec::new();
    for b in blocks {
        let piece = match b.kind.as_str() {
            // As before 0.1.18: an unknown game reads "Clip".
            "game" => util::sanitize_filename(p.game),
            "date" => p.when.format("%Y-%m-%d").to_string(),
            "time" => p.when.format("%H-%M-%S").to_string(),
            "length" => length_text(p.secs),
            "text" => clean(&b.text, MAX_TEXT),
            "number" => p.number.clone().unwrap_or_default(),
            "current" => clean(p.current, MAX_NAME),
            _ => String::new(),
        };
        if !piece.is_empty() {
            out.push(piece);
        }
    }
    let name: String = out.join(" ").chars().take(MAX_NAME).collect();
    let mut name = name.trim_end_matches(|c| c == '.' || c == ' ').to_string();
    if !name.is_empty() && reserved(&name) {
        name.push('_');
    }
    name
}

/// The name `blocks` make for a clip: a valid file name (without extension),
/// never empty. Blocks that make nothing (an empty pattern, only empty text)
/// give the default name.
pub fn build(blocks: &[NameBlock], p: &Parts) -> String {
    let name = assemble(blocks, p);
    if name.is_empty() {
        assemble(&default_blocks(), p)
    } else {
        name
    }
}

/// The title (and file name, before " (2)" is added for a clash) of a clip saved now.
pub fn clip_title(blocks: &[NameBlock], game: &str, secs: f64) -> String {
    build(blocks, &Parts { game, when: Local::now(), secs, number: None, current: "" })
}

// ---------------------------------------------------------------------------
// Bulk rename

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct BulkOpts {
    pub blocks: Vec<NameBlock>,
    /// First number of the Number block.
    pub start: u32,
    /// Least digits of the Number block (1 = "1", 3 = "001").
    pub pad: u32,
}

/// One clip in the preview: old → new.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PlanRow {
    pub id: String,
    pub old: String,
    pub new: String,
    /// The new name had to be made unique (" (2)").
    pub clash: bool,
    /// Why the clip is left as it is ("" = it is renamed).
    pub skip: String,
    /// Imported in place: the title changes, the other program's file keeps its name.
    pub title_only: bool,
}

pub struct Item {
    pub meta: ClipMeta,
    pub skip: String,
    pub title_only: bool,
}

const BUSY: &str = "This clip is being rendered";
const MISSING: &str = "The clip's file is missing";

fn when_of(ms: u64) -> DateTime<Local> {
    Local.timestamp_millis_opt(ms as i64).single().unwrap_or_else(Local::now)
}

fn key(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").to_lowercase()
}

fn split(path: &str) -> (PathBuf, String) {
    let p = Path::new(path);
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    (dir, ext)
}

/// The plan for `items` in their order: the new name of each, unique in its
/// folder the way Glint makes names unique (" (2)", " (3)", ...). `taken` says
/// whether a path is in use on disk; the batch's own files count as free (they
/// are moved aside before any is given its new name, so names can be swapped).
/// Each row comes with the clip's new path, None when its file keeps its name.
pub fn plan_items(items: &[Item], o: &BulkOpts, taken: &dyn Fn(&Path) -> bool) -> Vec<(PlanRow, Option<PathBuf>)> {
    let own: HashSet<String> = items.iter().filter(|i| i.skip.is_empty() && !i.title_only).map(|i| key(Path::new(&i.meta.path))).collect();
    let mut assigned: HashSet<String> = HashSet::new();
    let mut titles: HashSet<String> = HashSet::new(); // title-only clips: unique within the batch, per folder
    let mut n = o.start;
    let mut out = Vec::new();
    for it in items {
        let m = &it.meta;
        let mut row = PlanRow { id: m.id.clone(), old: m.title.clone(), new: m.title.clone(), clash: false, skip: it.skip.clone(), title_only: it.title_only };
        if !it.skip.is_empty() {
            out.push((row, None));
            continue;
        }
        let number = number_text(n, o.pad);
        n = n.saturating_add(1);
        let name = build(&o.blocks, &Parts { game: &m.game, when: when_of(m.created), secs: m.duration, number: Some(number), current: &m.title });
        let (dir, ext) = split(&m.path);
        let candidate = |i: u32| if i < 2 { name.clone() } else { format!("{name} ({i})") };
        let mut i = 1;
        if it.title_only {
            loop {
                let k = key(&dir.join(format!("{}.{ext}", candidate(i))));
                if titles.insert(k) {
                    break;
                }
                i += 1;
            }
            row.new = candidate(i);
            row.clash = i > 1;
            out.push((row, None));
            continue;
        }
        let path = loop {
            let p = dir.join(format!("{}.{ext}", candidate(i)));
            let k = key(&p);
            if !assigned.contains(&k) && (own.contains(&k) || !taken(&p)) {
                assigned.insert(k);
                break p;
            }
            i += 1;
        };
        row.new = candidate(i);
        row.clash = i > 1;
        let moved = path.as_os_str() != Path::new(&m.path).as_os_str();
        out.push((row, moved.then_some(path)));
    }
    out
}

/// The selected clips, in the order given (each once), with what stops them being renamed.
fn gather(core: &Core, ids: &[String]) -> Vec<Item> {
    let root = PathBuf::from(core.settings().clips_dir);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        let got = core.lib.lock().unwrap().get_saved(id);
        out.push(match got {
            Err(e) => Item { meta: ClipMeta { id: id.clone(), ..Default::default() }, skip: e, title_only: false },
            Ok(m) => {
                let title_only = m.kind == "import" && !in_clips_dir(&root, &m.path);
                let skip = if core.jobs.busy(id) {
                    BUSY.to_string()
                } else if !title_only && !Path::new(&m.path).is_file() {
                    MISSING.to_string()
                } else {
                    String::new()
                };
                Item { meta: m, skip, title_only }
            }
        });
    }
    out
}

/// The preview: old → new for every clip in `ids`, in that order.
pub fn plan(core: &Core, ids: &[String], o: &BulkOpts) -> Vec<PlanRow> {
    plan_items(&gather(core, ids), o, &|p| p.exists()).into_iter().map(|(r, _)| r).collect()
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct BulkDone {
    /// Clips whose name changed.
    pub renamed: usize,
    /// "<title> (<reason>)" for each clip left as it was.
    pub failed: Vec<String>,
    /// Hand to `undo` to put the old names back ("" when nothing changed).
    pub token: String,
    /// The changed clips as saved.
    pub clips: Vec<ClipMeta>,
}

struct Undo {
    token: String,
    /// (id, old title, old path, new path)
    items: Vec<(String, String, String, String)>,
}

/// One bulk rename runs at a time; the last one can be undone.
static RUN: Mutex<()> = Mutex::new(());
static LAST: Mutex<Option<Undo>> = Mutex::new(None);

fn rename_err(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(32) | Some(33) => "it is open in another program".into(),
        Some(5) => "access denied".into(),
        Some(2) | Some(3) => "its file is missing".into(),
        _ => e.to_string(),
    }
}

/// The first free "<stem> (n).<ext>" next to `p`.
fn unique_beside(p: &Path) -> PathBuf {
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Clip".into());
    let ext = p.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    util::unique_path(&dir, &stem, &ext)
}

fn aside(src: &Path) -> PathBuf {
    src.with_file_name(format!(".glint-rename-{}.tmp", util::new_id()))
}

/// Moves: (index, from, to). Every file is first moved aside, then given its
/// new name, so names can be swapped within the batch. A file that cannot be
/// moved aside keeps its name (its index is returned in the failures); one
/// whose new name was taken meanwhile gets the next free one. Returns the
/// path each index ended at, and the failures.
fn move_files(moves: &[(usize, PathBuf, PathBuf)]) -> (Vec<(usize, PathBuf)>, Vec<(usize, String)>) {
    let mut parked = Vec::new();
    let mut failed = Vec::new();
    for (i, from, to) in moves {
        let tmp = aside(from);
        match fs::rename(from, &tmp) {
            Ok(()) => parked.push((*i, from.clone(), tmp, to.clone())),
            Err(e) => failed.push((*i, rename_err(&e))),
        }
    }
    let mut done = Vec::new();
    for (i, from, tmp, to) in parked {
        let to = if to.exists() { unique_beside(&to) } else { to };
        match fs::rename(&tmp, &to) {
            Ok(()) => {
                util::log(&format!("bulk rename: {} -> {}", from.display(), to.display()));
                done.push((i, to));
            }
            Err(e) => {
                // Back to its old name; if another clip of the batch has taken it, the next free one
                // (then the clip is listed as renamed, to where its file really is).
                let back = if from.exists() { unique_beside(&from) } else { from.clone() };
                if fs::rename(&tmp, &back).is_ok() && back != from {
                    done.push((i, back));
                } else {
                    failed.push((i, rename_err(&e)));
                }
            }
        }
    }
    (done, failed)
}

/// Rename every clip in `ids` as the plan says. Clips that cannot be renamed
/// are left exactly as they were and listed in `failed`.
pub fn apply(core: &Core, ids: &[String], o: &BulkOpts) -> Result<BulkDone, String> {
    let _run = RUN.lock().unwrap_or_else(|e| e.into_inner());
    let items = gather(core, ids);
    if items.is_empty() {
        return Err("No clips are selected".into());
    }
    let plan = plan_items(&items, o, &|p| p.exists());
    let mut out = BulkDone::default();
    let mut moves = Vec::new();
    for (i, (row, dest)) in plan.iter().enumerate() {
        if !row.skip.is_empty() {
            out.failed.push(format!("{} ({})", row.old, row.skip));
        } else if let Some(d) = dest {
            moves.push((i, PathBuf::from(&items[i].meta.path), d.clone()));
        }
    }
    let (done, failed) = move_files(&moves);
    let failed_ix: HashSet<usize> = failed.iter().map(|(i, _)| *i).collect();
    for (i, why) in failed {
        out.failed.push(format!("{} ({why})", items[i].meta.title));
    }
    let mut undo = Vec::new();
    for (i, (row, dest)) in plan.iter().enumerate() {
        if !row.skip.is_empty() || failed_ix.contains(&i) {
            continue;
        }
        let m = &items[i].meta;
        let path = if dest.is_some() { done.iter().find(|(j, _)| *j == i).map(|(_, p)| p.to_string_lossy().into_owned()).unwrap_or_else(|| m.path.clone()) } else { m.path.clone() };
        // A name that had to change again in the second step: the title follows the file.
        let title = if dest.is_some() { Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| row.new.clone()) } else { row.new.clone() };
        if title == m.title && path == m.path {
            continue;
        }
        let (t2, p2) = (title.clone(), path.clone());
        if let Some(c) = core.lib.lock().unwrap().update(&m.id, |c| {
            c.title = t2;
            c.path = p2;
        }) {
            undo.push((m.id.clone(), m.title.clone(), m.path.clone(), path));
            out.clips.push(c);
            out.renamed += 1;
        }
    }
    if !undo.is_empty() {
        out.token = util::new_id();
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Undo { token: out.token.clone(), items: undo });
    }
    Ok(out)
}

/// Put back the names the bulk rename `token` replaced. A clip that has moved
/// or been renamed since keeps its file where it is and only gets its old title.
pub fn undo(core: &Core, token: &str) -> Result<BulkDone, String> {
    let _run = RUN.lock().unwrap_or_else(|e| e.into_inner());
    let rec = {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        match last.take() {
            Some(u) if !token.is_empty() && u.token == token => u,
            other => {
                *last = other;
                return Err("There is nothing to undo".into());
            }
        }
    };
    let mut out = BulkDone::default();
    let mut moves = Vec::new();
    let mut keep = Vec::new(); // (id, old title, path to keep)
    for (n, (id, old_title, old_path, new_path)) in rec.items.iter().enumerate() {
        let Ok(m) = core.lib.lock().unwrap().get_saved(id) else {
            out.failed.push(format!("{old_title} (Clip not found)"));
            continue;
        };
        if core.jobs.busy(id) {
            out.failed.push(format!("{} ({BUSY})", m.title));
            continue;
        }
        if old_path != new_path && key(Path::new(&m.path)) == key(Path::new(new_path)) && Path::new(&m.path).is_file() {
            moves.push((n, PathBuf::from(&m.path), PathBuf::from(old_path)));
        } else {
            keep.push((id.clone(), old_title.clone(), m.path.clone()));
        }
    }
    let (done, failed) = move_files(&moves);
    for (n, why) in failed {
        out.failed.push(format!("{} ({why})", rec.items[n].1));
    }
    for (n, path) in done {
        keep.push((rec.items[n].0.clone(), rec.items[n].1.clone(), path.to_string_lossy().into_owned()));
    }
    for (id, title, path) in keep {
        if let Some(c) = core.lib.lock().unwrap().update(&id, |c| {
            c.title = title;
            c.path = path;
        }) {
            out.clips.push(c);
            out.renamed += 1;
        }
    }
    Ok(out)
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
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        if ok {
            self.passed += 1;
        } else {
            self.failed += 1;
        }
        let tag = if ok { "PASS" } else { "FAIL" };
        let detail = detail.as_ref();
        if detail.is_empty() {
            self.line(&format!("{tag} {name}"));
        } else {
            self.line(&format!("{tag} {name} - {detail}"));
        }
    }
}

/// `glint.exe --naming-test <report.txt>`: name patterns, uniqueness, illegal
/// characters, then bulk rename and undo on made-up clips (needs GLINT_DATA_DIR).
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0, passed: 0 };
    let _ = fs::remove_file(&r.path);
    pure_tests(&mut r);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("bulk rename refused", false, "set GLINT_DATA_DIR so the real library is never touched");
    } else {
        bulk_tests(&mut r);
    }
    let (passed, failed) = (r.passed, r.failed);
    r.line(&format!("DONE passed={passed} failed={failed}"));
}

fn pure_tests(r: &mut Report) {
    let when = Local.with_ymd_and_hms(2026, 11, 14, 13, 35, 44).unwrap();
    let p = |game: &'static str| Parts { game, when, secs: 30.2, number: None, current: "" };
    let b = |kinds: &[&str]| kinds.iter().map(|k| NameBlock::of(k)).collect::<Vec<_>>();

    let today = format!("{} {}", util::sanitize_filename("Overwatch"), when.format("%Y-%m-%d %H-%M-%S"));
    let got = build(&default_blocks(), &p("Overwatch"));
    r.check("default blocks = the name before 0.1.18", got == today && got == "Overwatch 2026-11-14 13-35-44", &got);
    let long_game = "A Very Long Game Name That Goes On And On And On And On And On For Ever";
    let got = build(&default_blocks(), &p(long_game));
    let today = format!("{} {}", util::sanitize_filename(long_game), when.format("%Y-%m-%d %H-%M-%S"));
    r.check("default blocks = the old name for a long game name too", got == today, &got);
    let got = build(&default_blocks(), &p(""));
    r.check("unknown game reads Clip, as before", got == "Clip 2026-11-14 13-35-44", &got);
    let got = build(&b(&["game", "time"]), &p("Overwatch"));
    r.check("(Game) (Time) = 'Overwatch 13-35-44'", got == "Overwatch 13-35-44", &got);
    let got = build(&b(&["time", "date", "game"]), &p("Overwatch"));
    r.check("blocks in any order", got == "13-35-44 2026-11-14 Overwatch", &got);
    let got = build(&[NameBlock::of("game"), NameBlock::of("length")], &p("Overwatch"));
    r.check("length block", got == "Overwatch 30s", &got);
    let lens = [length_text(0.0), length_text(29.6), length_text(59.4), length_text(60.0), length_text(90.0), length_text(605.0), length_text(f64::NAN)];
    r.check("length text", lens == ["0s", "30s", "59s", "1m", "1m30s", "10m05s", "0s"], format!("{lens:?}"));
    let got = build(&[], &p("Overwatch"));
    r.check("empty pattern = the default", got == "Overwatch 2026-11-14 13-35-44", &got);
    let got = build(&[NameBlock::text("  "), NameBlock::text("..."), NameBlock::of("bogus")], &p("Overwatch"));
    r.check("a pattern that makes nothing = the default", got == "Overwatch 2026-11-14 13-35-44", &got);
    let got = build(&[NameBlock::text("my  best:play?"), NameBlock::of("game"), NameBlock::text("<win>")], &p("Overwatch"));
    r.check("illegal characters in text become spaces", got == "my best play Overwatch win", &got);
    let got = build(&[NameBlock::text("ranked"), NameBlock::text("..dot..")], &p("Overwatch"));
    r.check("text never ends or starts with a dot", got == "ranked dot", &got);
    let got = build(&[NameBlock::of("game")], &p("Over/watch: 2?"));
    r.check("game name made safe as before", got == "Over watch 2", &got);
    let got = build(&[NameBlock::text("con")], &p("x"));
    let got2 = build(&[NameBlock::text("LPT1")], &p("x"));
    let got3 = build(&[NameBlock::text("COM0"), ], &p("x"));
    r.check("Windows device names are not file names", got == "con_" && got2 == "LPT1_" && got3 == "COM0", format!("{got} {got2} {got3}"));
    let many: Vec<NameBlock> = (0..8).map(|_| NameBlock::text(&"x".repeat(59))).chain([NameBlock::text("tail. ")]).collect();
    let got = build(&many, &p("x"));
    r.check("a long pattern is cut to 120 characters, no space or dot at the end", got.chars().count() <= MAX_NAME && !got.ends_with(' ') && !got.ends_with('.'), format!("{} chars", got.chars().count()));
    let got = build(&[NameBlock::of("game"), NameBlock::text("오버워치 Ωmega")], &p("Valorant"));
    r.check("letters of any language kept", got == "Valorant 오버워치 Ωmega", &got);
    let got = build(&[NameBlock::of("number"), NameBlock::of("current")], &p("x"));
    r.check("number and current name make nothing for a new clip (default used)", got == "x 2026-11-14 13-35-44", &got);
    let got = build(&[NameBlock::of("current"), NameBlock::of("number")], &Parts { game: "x", when, secs: 1.0, number: Some(number_text(7, 3)), current: "old: name" });
    r.check("bulk blocks: current name and padded number", got == "old name 007", &got);
    r.check("number padding", number_text(1, 1) == "1" && number_text(12, 3) == "012" && number_text(1234, 2) == "1234" && number_text(5, 0) == "5", "");
    let s: crate::settings::Settings = serde_json::from_str("{}").unwrap_or_default();
    r.check("settings without clip_name get the default blocks", s.clip_name == default_blocks(), format!("{:?}", s.clip_name));
    let json = serde_json::to_string(&vec![NameBlock::of("game"), NameBlock::text("ranked")]).unwrap_or_default();
    let back: Vec<NameBlock> = serde_json::from_str(&json).unwrap_or_default();
    r.check("blocks save and load", json == r#"[{"kind":"game"},{"kind":"text","text":"ranked"}]"# && back.len() == 2 && back[1].text == "ranked", &json);
    let odd: Vec<NameBlock> = serde_json::from_str(r#"[{"kind":"game","extra":1},{}]"#).unwrap_or_default();
    r.check("odd saved blocks load without failing", odd.len() == 2 && build(&odd, &p("Overwatch")) == "Overwatch", format!("{odd:?}"));

    // A new clip's file: unique as before (two clips in one second, a pattern without date or time).
    let dir = std::env::temp_dir().join(format!("glint-naming-{}", util::new_id()));
    let _ = fs::create_dir_all(&dir);
    let title = clip_title(&[NameBlock::of("game")], "Overwatch", 30.0);
    let a = crate::clip::reserve_path(&dir, &title, "mp4");
    let b2 = crate::clip::reserve_path(&dir, &title, "mp4");
    let c = crate::clip::reserve_path(&dir, &title, "mp4");
    let names: Vec<String> = [&a, &b2, &c].iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
    r.check("new clips with the same name get (2), (3)", names == ["Overwatch.mp4", "Overwatch (2).mp4", "Overwatch (3).mp4"], format!("{names:?}"));
    let now = clip_title(&default_blocks(), "Overwatch", 30.0);
    r.check("a new clip's default title has today's date and time", now.starts_with("Overwatch ") && now.len() == "Overwatch 2026-11-14 13-35-44".len(), &now);
    let _ = fs::remove_dir_all(&dir);
}

fn bulk_tests(r: &mut Report) {
    let data = util::data_dir();
    let _ = fs::remove_file(data.join("library.json"));
    let root = data.join("naming clips");
    let other = data.join("other program");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&other);
    let dir = root.join("G");
    let _ = fs::create_dir_all(&dir);
    let _ = fs::create_dir_all(&other);
    let core = Core::new();
    core.settings.lock().unwrap().clips_dir = root.to_string_lossy().into_owned();
    core.jobs.connect(&core);
    let base_ms = Local.with_ymd_and_hms(2026, 11, 14, 13, 35, 44).unwrap().timestamp_millis() as u64;
    let add = |name: &str, folder: &Path, kind: &str, write: bool, k: u64| -> String {
        let p = folder.join(format!("{name}.mp4"));
        if write {
            let _ = fs::write(&p, name.as_bytes());
        }
        let id = format!("id-{name}");
        core.lib.lock().unwrap().add(ClipMeta { id: id.clone(), path: p.to_string_lossy().into_owned(), title: format!("Old {name}"), game: "Overwatch".into(), created: base_ms + k * 1000, duration: 30.0, kind: kind.into(), folder: "G".into(), ..Default::default() });
        id
    };
    let a = add("a", &dir, "clip", true, 0);
    let b = add("b", &dir, "clip", true, 1);
    let c = add("c", &dir, "clip", true, 2);
    let d = add("d", &other, "import", true, 3); // imported in place
    let e = add("e", &dir, "clip", true, 4); // being rendered
    let f = add("f", &dir, "clip", false, 5); // file missing
    let _ = fs::write(dir.join("Overwatch.mp4"), b"not in the batch");
    let job = core.jobs.enqueue(&e, "busy", "blur", Box::new(|stop: &std::sync::atomic::AtomicBool, _p: &mut dyn FnMut(f32)| {
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err("cancelled".into())
    }));
    r.check("a job holds clip e", job.is_ok() && core.jobs.busy(&e), "");
    let meta = |id: &str| core.lib.lock().unwrap().get(id).unwrap_or_default();
    let fname = |id: &str| Path::new(&meta(id).path).file_name().unwrap_or_default().to_string_lossy().into_owned();
    let body = |id: &str| fs::read_to_string(&meta(id).path).unwrap_or_default();
    let ids: Vec<String> = [&c, &a, &b, &d, &e, &f, &a].iter().map(|s| s.to_string()).collect();

    let game_only = BulkOpts { blocks: vec![NameBlock::of("game")], start: 1, pad: 1 };
    let pl = plan(&core, &ids, &game_only);
    let news: Vec<&str> = pl.iter().map(|x| x.new.as_str()).collect();
    r.check("plan: one row per clip, in the order selected, each once", pl.len() == 6 && pl[0].id == c && pl[1].id == a && pl[3].id == d, format!("{:?}", pl.iter().map(|x| &x.id).collect::<Vec<_>>()));
    r.check("plan: names that clash are made unique like Glint does", news[..4] == ["Overwatch (2)", "Overwatch (3)", "Overwatch (4)", "Overwatch"] && pl[0].clash && pl[1].clash && !pl[3].clash, format!("{news:?}"));
    r.check("plan: in-place import changes its title only", pl[3].title_only && pl[3].skip.is_empty(), format!("{:?}", pl[3]));
    r.check("plan: a clip being rendered is refused", pl[4].skip == BUSY && pl[4].new == pl[4].old, &pl[4].skip);
    r.check("plan: a clip whose file is missing is refused", pl[5].skip == MISSING, &pl[5].skip);
    r.check("plan: nothing on disk changed", fname(&a) == "a.mp4" && meta(&a).title == "Old a", "");

    let done = apply(&core, &ids, &game_only);
    let Ok(done) = done else {
        r.check("bulk rename ran", false, format!("{done:?}"));
        return;
    };
    r.check("bulk rename: 4 renamed, 2 refused, undo offered", done.renamed == 4 && done.failed.len() == 2 && !done.token.is_empty(), format!("{} {:?}", done.renamed, done.failed));
    r.check("bulk rename: files and titles are one name", fname(&c) == "Overwatch (2).mp4" && meta(&c).title == "Overwatch (2)" && fname(&a) == "Overwatch (3).mp4" && fname(&b) == "Overwatch (4).mp4", format!("{} {} {}", fname(&c), fname(&a), fname(&b)));
    r.check("bulk rename: each file kept its own contents", body(&c) == "c" && body(&a) == "a" && body(&b) == "b", "");
    r.check("bulk rename: old files are gone, the other file untouched", !dir.join("a.mp4").exists() && !dir.join("c.mp4").exists() && fs::read_to_string(dir.join("Overwatch.mp4")).unwrap_or_default() == "not in the batch", "");
    r.check("bulk rename: in-place import keeps its file name", fname(&d) == "d.mp4" && other.join("d.mp4").is_file() && meta(&d).title == "Overwatch", format!("{} {}", fname(&d), meta(&d).title));
    r.check("bulk rename: refused clips untouched", fname(&e) == "e.mp4" && meta(&e).title == "Old e" && meta(&f).title == "Old f", "");
    let leftovers = fs::read_dir(&dir).map(|rd| rd.flatten().filter(|x| x.file_name().to_string_lossy().starts_with(".glint-rename")).count()).unwrap_or(99);
    r.check("bulk rename: no temporary files left", leftovers == 0, format!("{leftovers}"));

    r.check("undo: a wrong token is refused", undo(&core, "nope").is_err(), "");
    let un = undo(&core, &done.token);
    r.check("undo: the old names are back", un.as_ref().map(|u| u.renamed == 4).unwrap_or(false) && fname(&a) == "a.mp4" && fname(&b) == "b.mp4" && fname(&c) == "c.mp4" && meta(&a).title == "Old a" && meta(&d).title == "Old d", format!("{un:?}"));
    r.check("undo: contents still match", body(&a) == "a" && body(&b) == "b" && body(&c) == "c", "");
    r.check("undo: only once", undo(&core, &done.token).is_err(), "");

    // Numbers in the order selected, then the same names in the reverse order: a swap.
    let numbered = BulkOpts { blocks: vec![NameBlock::text("play"), NameBlock::of("number")], start: 1, pad: 1 };
    let abc: Vec<String> = vec![a.clone(), b.clone(), c.clone()];
    let _ = apply(&core, &abc, &numbered);
    r.check("numbers follow the selection order", fname(&a) == "play 1.mp4" && fname(&b) == "play 2.mp4" && fname(&c) == "play 3.mp4", format!("{} {} {}", fname(&a), fname(&b), fname(&c)));
    let cba: Vec<String> = vec![c.clone(), b.clone(), a.clone()];
    let pl = plan(&core, &cba, &numbered);
    r.check("swapping names within the batch is no clash", pl.iter().all(|x| !x.clash), format!("{pl:?}"));
    let sw = apply(&core, &cba, &numbered);
    r.check("swap: names swapped, contents follow their clips", fname(&c) == "play 1.mp4" && fname(&a) == "play 3.mp4" && body(&c) == "c" && body(&a) == "a" && body(&b) == "b", format!("{sw:?}"));

    let padded = BulkOpts { blocks: vec![NameBlock::of("game"), NameBlock::of("date"), NameBlock::of("number"), NameBlock::text("a/b")], start: 9, pad: 3 };
    let pl = plan(&core, &abc, &padded);
    let news: Vec<&str> = pl.iter().map(|x| x.new.as_str()).collect();
    r.check("each clip's own date, padded numbers, illegal characters", news == ["Overwatch 2026-11-14 009 a b", "Overwatch 2026-11-14 010 a b", "Overwatch 2026-11-14 011 a b"], format!("{news:?}"));
    let cur = BulkOpts { blocks: vec![NameBlock::of("current"), NameBlock::of("time"), NameBlock::of("length")], start: 1, pad: 1 };
    let pl = plan(&core, &[a.clone()], &cur);
    r.check("current name, the clip's own time and length", pl[0].new == "play 3 13-35-44 30s", &pl[0].new);

    // Letter case only.
    let upper = BulkOpts { blocks: vec![NameBlock::text("PLAY"), NameBlock::of("number")], start: 3, pad: 1 };
    let _ = apply(&core, &[a.clone()], &upper);
    let listed = fs::read_dir(&dir).map(|rd| rd.flatten().any(|x| x.file_name().to_string_lossy() == "PLAY 3.mp4")).unwrap_or(false);
    r.check("a change of letter case only renames the file", listed && meta(&a).title == "PLAY 3", &fname(&a));

    // A single rename of a clip imported in place changes its title only.
    let one = core.rename_clip(&d, "New: name");
    r.check("single rename of an in-place import keeps the other program's file", one.is_ok() && fname(&d) == "d.mp4" && other.join("d.mp4").is_file() && meta(&d).title == "New: name", format!("{one:?}"));
    let one = core.rename_clip(&e, "x");
    r.check("single rename of a clip being rendered is refused", one.is_err() && fname(&e) == "e.mp4", "");

    if let Ok(j) = job {
        core.jobs.cancel(&j.id);
    }
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&other);
}
