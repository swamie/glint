//! Editor projects: an "Edit together" draft that is KEPT, plus the editor's
//! saved state, so an editing session survives closing the editor and Glint.
//!
//! 0.1.38 (CONTRACT_0138): a project made from now on is VIRTUAL (`virtual` in
//! projects.json): only its list of library clips and the saved edit, no draft
//! file. Adding is instant (`add` -> `add_now`), "Edit together" makes one at
//! once (`join_clips`), and `open` (`project_open`) hands the editor the project
//! as one clip made of `sources` laid end to end ("project time", `virtual_clip`).
//! A project made before keeps its draft and everything below, unchanged.
//!
//! - `<data_dir>\projects.json`: every project (id, name, times, the library
//!   clips added, `exported`). The draft itself is a library entry (`draft`
//!   true, kind "joined", id = the project id) whose file is in
//!   `<clips_dir>\.drafts`; it never shows in `list_clips`.
//! - `<data_dir>\projects\<id>.state.json`: the editor's state, an opaque text.
//!
//! Adding clips is a job of kind "join" (join.rs does the joining):
//! - an empty project: the clips joined as "Edit together" does; one clip alone
//!   is copied as it is (lossless);
//! - a project with a draft: [the draft, the new clips] joined into a NEW file
//!   (its path changes, so caches keyed by path refresh). The old `joins` and
//!   `join_names` stay exactly as they were and the new ones are appended, so
//!   every source second of the old draft keeps its place and saved edits stay valid.
//! When the job ends well: "project-changed" { project, job_id, added, mode,
//! secs, starts, titles } ("clip-joined" of 0.1.14 is no longer sent).
//! "projects-changed" (no payload) whenever the list or a project changes.
//!
//! At start-up (`sweep_at_start`) drafts that belong to a project are kept;
//! every other draft and file in `.drafts`, and every state file of no project, is removed.
use crate::core::Core;
use crate::jobs::{Job, Runner};
use crate::join::{self, Mode};
use crate::library::{self, ClipMeta, Library};
use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Most library clips in one project.
// 0.1.41: a project references its clips, so the old joining limits (20 clips, 30 minutes: join.rs) no longer apply to it.
pub const MAX_CLIPS: usize = 500;
/// Most seconds of video in one project.
pub const MAX_TOTAL_SECS: f64 = 4.0 * 3600.0;
/// The name of a project made without one.
pub const DEFAULT_NAME: &str = "New project";

pub const NOT_FOUND: &str = "Project not found";
pub const NOTHING: &str = "Pick at least one clip to add";
pub const TOO_MANY: &str = "A project takes at most 500 clips";
pub const TOO_LONG: &str = "A project takes at most 4 hours of clips";
pub const NO_NAME: &str = "Type a name for the project";

pub const EMPTY: &str = "This project has no clips yet";
pub const NO_DRAFT: &str = "The working copy of this project is missing";

/// A library clip added to a project. Since 0.1.38 it keeps what is needed to
/// show the clip when it has gone from the library (size, picture, tracks).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct ProjectClip {
    pub id: String,
    pub title: String,
    /// Its length when it was added: the length of its stretch of project time.
    pub duration: f64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio_tracks: u32,
    pub game: String,
}

fn pclip(c: &ClipMeta) -> ProjectClip {
    ProjectClip { id: c.id.clone(), title: c.title.clone(), duration: c.duration, size: c.size, width: c.width, height: c.height, fps: c.fps, audio_tracks: c.audio_tracks, game: c.game.clone() }
}

/// What projects.json keeps of a project.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Stored {
    id: String,
    name: String,
    created: u64,
    modified: u64,
    clips: Vec<ProjectClip>,
    exported: bool,
    /// 0.1.38: the project is its clip list (no draft file). Absent (false) in a
    /// project made by 0.1.37 or earlier: that one keeps its draft and old ways.
    #[serde(rename = "virtual")]
    virt: bool,
    /// 0.1.41 (CONTRACT_0141F, F1): a project made before 0.1.38 that got clips since: its draft working file,
    /// as it was, is its first source (`base`), and `clips[..base_clips]` are the clips inside it. Never rewritten.
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<Base>,
    base_clips: usize,
}

/// The draft of an old project as its first source (F1): what it was when the project became one of references.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Base {
    duration: f64,
    /// Where each clip inside it after the first starts, and every clip's title (`joins.len() + 1`), as the draft had them.
    joins: Vec<f64>,
    names: Vec<String>,
    width: u32,
    height: u32,
    fps: u32,
    audio_tracks: u32,
    size: u64,
}

/// A project as the UI gets it.
#[derive(Serialize, Clone, Debug, Default)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Unix milliseconds.
    pub created: u64,
    pub modified: u64,
    /// The draft of a project made before 0.1.38 (draft: true, kind "joined", joins, join_names); None while empty and for a virtual project.
    #[serde(skip)]
    pub clip: Option<ClipMeta>,
    /// What the UI gets as `clip`: the draft above, or for a virtual project what
    /// `project_open` returns (null while it has no clips).
    #[serde(rename = "clip")]
    pub clip_json: Option<Value>,
    /// 0.1.38: a project of references (`clip_json.virtual`), no draft file.
    #[serde(rename = "virtual")]
    pub virt: bool,
    /// The library clips added, in the order added.
    pub clips: Vec<ProjectClip>,
    /// Something was exported from it since its last change.
    pub exported: bool,
    /// Bytes of the draft file.
    pub size: u64,
    /// A queued or running job is building its draft.
    pub busy: bool,
}

// ---------------------------------------------------------------------------
// The store

/// Held while projects.json is read and written.
static LOCK: Mutex<()> = Mutex::new(());

fn file() -> PathBuf {
    util::data_dir().join("projects.json")
}

/// projects.json exists but could not be read or understood (a sharing violation that outlasted the retries,
/// a file from a newer version): nothing is saved over it until Glint starts again (0.1.41, R3-9: an empty
/// list read that way was saved back, wiping every project).
static UNREADABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn load() -> Vec<Stored> {
    let path = file();
    if !path.exists() {
        return Vec::new();
    }
    let mut why = String::new();
    for attempt in 0..4 {
        match fs::read_to_string(&path) {
            Ok(t) => match serde_json::from_str::<Vec<Stored>>(&t) {
                Ok(v) => return v,
                Err(e) => {
                    why = format!("not understood ({e})");
                    break; // the same text again would fail the same way
                }
            },
            Err(e) => why = format!("not readable ({e})"),
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }
    if !UNREADABLE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        util::log(&format!("projects: {} is {why}; it is left as it is and nothing is saved over it", path.display()));
    }
    Vec::new()
}

fn save(list: &[Stored]) {
    if UNREADABLE.load(std::sync::atomic::Ordering::SeqCst) {
        util::log("projects: not saved (projects.json could not be read; it is kept as it was)");
        return;
    }
    if let Ok(t) = serde_json::to_string(list) {
        let tmp = file().with_extension("json.tmp");
        // The last good file stays as projects.json.bak.
        if file().is_file() {
            let _ = fs::copy(file(), file().with_extension("json.bak"));
        }
        if fs::write(&tmp, t).is_ok() && fs::rename(&tmp, file()).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// Tests: forget that projects.json was unreadable.
pub fn test_reset_unreadable() {
    UNREADABLE.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Change project `id` in the store. None when there is no such project.
fn edit<R>(id: &str, f: impl FnOnce(&mut Stored) -> R) -> Option<R> {
    let _g = LOCK.lock().unwrap();
    let mut list = load();
    let p = list.iter_mut().find(|p| p.id == id)?;
    let out = f(p);
    save(&list);
    Some(out)
}

fn stored(id: &str) -> Option<Stored> {
    let _g = LOCK.lock().unwrap();
    load().into_iter().find(|p| p.id == id)
}

/// True when `id` is a project.
pub fn exists(id: &str) -> bool {
    stored(id).is_some()
}

/// The ids of every project.
pub fn ids() -> Vec<String> {
    let _g = LOCK.lock().unwrap();
    load().into_iter().map(|p| p.id).collect()
}

fn states_dir() -> PathBuf {
    util::data_dir().join("projects")
}

/// `<data_dir>\projects\<id>.state.json`; None for an id that is not a plain name.
fn state_file(id: &str) -> Option<PathBuf> {
    let plain = !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| states_dir().join(format!("{id}.state.json")))
}

// ---------------------------------------------------------------------------
// Jobs that are building a project's draft

struct Pending {
    token: u64,
    project: String,
    /// The job's id once `Jobs::enqueue` has answered ("" until then).
    job_id: String,
    clips: usize,
    secs: f64,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static TOKEN: AtomicU64 = AtomicU64::new(1);

/// The pending adds of `project` whose job is still queued or running.
fn pending_of(core: &Core, project: &str) -> Vec<(usize, f64)> {
    let jobs = core.jobs.list();
    let live = |id: &str| id.is_empty() || jobs.iter().any(|j| j.id == id && matches!(j.state.as_str(), "queued" | "running"));
    let mut p = PENDING.lock().unwrap();
    p.retain(|x| live(&x.job_id));
    p.iter().filter(|x| x.project == project).map(|x| (x.clips, x.secs)).collect()
}

fn unpend(token: u64) {
    PENDING.lock().unwrap().retain(|x| x.token != token);
}

/// True while a job builds `id`'s draft, or any job reads the draft.
fn busy(core: &Core, id: &str) -> bool {
    !pending_of(core, id).is_empty() || core.jobs.busy(id)
}

// ---------------------------------------------------------------------------
// Events (the self-test can record them)

static RECORD: Mutex<Option<Vec<(String, Value)>>> = Mutex::new(None);

fn emit(core: &Core, event: &str, payload: Value) {
    if let Some(log) = RECORD.lock().unwrap().as_mut() {
        log.push((event.to_string(), payload.clone()));
    }
    core.emit(event, payload);
}

fn changed(core: &Core) {
    emit(core, "projects-changed", Value::Null);
}

// ---------------------------------------------------------------------------
// Commands

fn view(lib: &Library, p: Stored, busy: bool) -> Project {
    if p.virt {
        let clip_json = virtual_clip(lib, &p);
        return Project { id: p.id, name: p.name, created: p.created, modified: p.modified, clip: None, clip_json, virt: true, clips: p.clips, exported: p.exported, size: 0, busy };
    }
    let clip = lib.get(&p.id).filter(|c| c.draft && Path::new(&c.path).is_file());
    let size = clip.as_ref().map(|c| fs::metadata(&c.path).map(|m| m.len()).unwrap_or(c.size)).unwrap_or(0);
    let clip_json = clip.as_ref().and_then(|c| serde_json::to_value(c).ok());
    Project { id: p.id, name: p.name, created: p.created, modified: p.modified, clip, clip_json, virt: false, clips: p.clips, exported: p.exported, size, busy }
}

/// The length of project clip `c`'s stretch of project time (its length when added).
fn span(c: &ProjectClip) -> f64 {
    c.duration.max(0.0)
}

/// Project time of a virtual project: its draft source (F1), then every later clip's span.
fn proj_len(p: &Stored) -> f64 {
    match &p.base {
        Some(b) => b.duration.max(0.0) + p.clips[p.base_clips.min(p.clips.len())..].iter().map(span).sum::<f64>(),
        None => p.clips.iter().map(span).sum(),
    }
}

/// A virtual project as the clip the editor opens (CONTRACT_0138, "What the editor
/// opens"): its clips end to end in the order added, each a source with its start
/// in project time. A clip the library no longer has, or whose file is gone, is
/// `missing` (shown from what the project kept of it). None while it has no clips.
fn virtual_clip(lib: &Library, p: &Stored) -> Option<Value> {
    if p.clips.is_empty() && p.base.is_none() {
        return None;
    }
    let mut sources: Vec<Value> = Vec::with_capacity(p.clips.len() + 1);
    let mut joins: Vec<f64> = Vec::with_capacity(p.clips.len());
    let mut names: Vec<String> = Vec::with_capacity(p.clips.len());
    let (mut at, mut size) = (0.0_f64, 0_u64);
    let mut first: Option<(u32, u32, u32, u32, String)> = None;
    let mut thumb = String::new();
    // F1 (0.1.41): an old project's draft file is source 0, as it is: its own clips inside it are `inner`
    // ({ at, title }: seconds in that file), and every border of them is in `joins` / `join_names`, so
    // project time for all that was there is the draft's own time and the saved edit fits unchanged.
    if let Some(b) = &p.base {
        let d = lib.get(&p.id).filter(|c| c.draft);
        let live = d.as_ref().filter(|c| Path::new(&c.path).is_file());
        let name = |i: usize| b.names.get(i).filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("Clip {}", i + 1));
        let mut inner = vec![json!({ "at": 0.0, "title": name(0) })];
        joins.push(0.0);
        names.push(name(0));
        for (i, &j) in b.joins.iter().enumerate() {
            inner.push(json!({ "at": j, "title": name(i + 1) }));
            joins.push(j);
            names.push(name(i + 1));
        }
        let game = d.as_ref().map(|c| c.game.clone()).unwrap_or_default();
        first = Some((b.width, b.height, b.fps, b.audio_tracks, game));
        thumb = live.map(|c| c.thumb.clone()).unwrap_or_default();
        size += b.size;
        sources.push(json!({
            "clip": p.id, "path": live.map(|c| c.path.clone()).unwrap_or_default(), "title": p.name, "duration": b.duration, "start": 0.0,
            "width": b.width, "height": b.height, "fps": b.fps, "audio_tracks": b.audio_tracks, "thumb": thumb, "missing": live.is_none(),
            "draft": true, "inner": inner,
        }));
        at = b.duration;
    }
    for pc in &p.clips[p.base_clips.min(p.clips.len())..] {
        let live = lib.get(&pc.id).filter(|c| !c.draft && Path::new(&c.path).is_file());
        let (path, title, w, h, fps, tracks, th, game, sz) = match &live {
            Some(c) => (c.path.clone(), c.title.clone(), c.width, c.height, c.fps, c.audio_tracks, c.thumb.clone(), c.game.clone(), c.size),
            None => (String::new(), pc.title.clone(), pc.width, pc.height, pc.fps, pc.audio_tracks, String::new(), pc.game.clone(), pc.size),
        };
        let d = span(pc);
        if first.is_none() {
            first = Some((w, h, fps, tracks, game));
        }
        if thumb.is_empty() && !th.is_empty() {
            thumb = th.clone();
        }
        size += sz;
        joins.push(at);
        names.push(title.clone());
        sources.push(json!({
            "clip": pc.id, "path": path, "title": title, "duration": d, "start": at,
            "width": w, "height": h, "fps": fps, "audio_tracks": tracks, "thumb": th, "missing": live.is_none(),
        }));
        at += d;
    }
    let (width, height, fps, audio_tracks, game) = first.unwrap_or_default();
    Some(json!({
        "id": p.id, "draft": true, "virtual": true, "kind": "joined",
        "title": p.name, "game": game, "created": p.created, "duration": at,
        "width": width, "height": height, "fps": fps,
        "path": "", "joins": joins, "join_names": names,
        "audio_tracks": audio_tracks, "sources": sources,
        // Not in the design's list, for the card and the viewer's info line: the first clip's thumbnail, the sources' bytes.
        "thumb": thumb, "size": size, "folder": "", "favorite": false,
    }))
}

/// `project_open`: the clip the editor opens for project `id`. A virtual project:
/// `virtual_clip`. A project made before 0.1.38: its draft, exactly as before.
pub fn open(core: &Core, id: &str) -> Result<Value, String> {
    let p = stored(id).ok_or(NOT_FOUND)?;
    let lib = core.lib.lock().unwrap();
    if p.virt {
        return virtual_clip(&lib, &p).ok_or_else(|| EMPTY.into());
    }
    match lib.get(id).filter(|c| c.draft && Path::new(&c.path).is_file()) {
        Some(c) => serde_json::to_value(&c).map_err(|e| e.to_string()),
        None => Err(if p.clips.is_empty() { EMPTY.into() } else { NO_DRAFT.into() }),
    }
}

/// Every project, most recently modified first.
pub fn list(core: &Core) -> Vec<Project> {
    let all = {
        let _g = LOCK.lock().unwrap();
        load()
    };
    // The job list is read before the library is locked (never both at once from here).
    let flags: Vec<bool> = all.iter().map(|p| busy(core, &p.id)).collect();
    let lib = core.lib.lock().unwrap();
    let mut out: Vec<Project> = all.into_iter().zip(flags).map(|(p, b)| view(&lib, p, b)).collect();
    drop(lib);
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.created.cmp(&a.created)));
    out
}

pub fn get(core: &Core, id: &str) -> Option<Project> {
    let p = stored(id)?;
    let b = busy(core, id);
    let lib = core.lib.lock().unwrap();
    Some(view(&lib, p, b))
}

fn clean_name(name: &str) -> String {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(100).collect();
    n.trim().to_string()
}

/// A new project in the store. `virt`: one of references (every project made from 0.1.38 on; false only in the self-test's old-style ones).
fn make(name: &str, virt: bool) -> Stored {
    let name = clean_name(name);
    let now = util::unix_ms();
    let p = Stored { id: util::new_id(), name: if name.is_empty() { DEFAULT_NAME.into() } else { name }, created: now, modified: now, clips: Vec::new(), exported: false, virt, base: None, base_clips: 0 };
    let _g = LOCK.lock().unwrap();
    let mut list = load();
    list.push(p.clone());
    save(&list);
    p
}

/// A new, empty project.
pub fn create(core: &Core, name: &str) -> Project {
    let p = make(name, true);
    util::log(&format!("projects: created {} \"{}\"", p.id, p.name));
    changed(core);
    get(core, &p.id).unwrap_or_default()
}

pub fn rename(core: &Core, id: &str, name: &str) -> Result<Project, String> {
    let name = clean_name(name);
    if name.is_empty() {
        return Err(NO_NAME.into());
    }
    edit(id, |p| p.name = name.clone()).ok_or(NOT_FOUND)?;
    // The draft's title follows, so what is exported from it is named after the project.
    core.lib.lock().unwrap().update(id, |c| {
        if c.draft {
            c.title = name.clone();
        }
    });
    changed(core);
    get(core, id).ok_or_else(|| NOT_FOUND.into())
}

/// The editor's saved state of project `id`, if any.
pub fn state(id: &str) -> Option<String> {
    fs::read_to_string(state_file(id)?).ok()
}

/// Save the editor's state: the project is modified now and not exported since.
pub fn save_state(core: &Core, id: &str, text: &str) -> Result<Project, String> {
    let path = state_file(id).ok_or(NOT_FOUND)?;
    if !exists(id) {
        return Err(NOT_FOUND.into());
    }
    let _ = fs::create_dir_all(states_dir());
    let tmp = path.with_extension("tmp");
    if let Err(e) = fs::write(&tmp, text) {
        util::log(&format!("projects: saving the state of {id}: {e}"));
        return Err("Could not save the project".into());
    }
    if fs::rename(&tmp, &path).is_err() {
        let _ = fs::remove_file(&tmp);
        return Err("Could not save the project".into());
    }
    edit(id, |p| {
        p.modified = util::unix_ms().max(p.modified);
        p.exported = false;
    })
    .ok_or(NOT_FOUND)?;
    changed(core);
    get(core, id).ok_or_else(|| NOT_FOUND.into())
}

/// Something was exported from draft `id`: its project (if it has one) is "exported".
pub fn mark_exported(core: &Core, id: &str) {
    if edit(id, |p| p.exported = true).is_some() {
        changed(core);
    }
}

/// Remove the project's entry, draft (entry, file, thumbnail) and state. No checks, no event.
fn remove(core: &Core, id: &str) {
    {
        let _g = LOCK.lock().unwrap();
        let mut list = load();
        list.retain(|p| p.id != id);
        save(&list);
    }
    let meta = {
        let mut lib = core.lib.lock().unwrap();
        if lib.get(id).is_some_and(|c| c.draft) { lib.remove(id) } else { None }
    };
    if let Some(m) = meta {
        library::remove_draft_files(&m);
    }
    if let Some(f) = state_file(id) {
        let _ = fs::remove_file(f);
    }
}

/// Delete project `id`: false while a job uses it or when there is no such project.
pub fn delete(core: &Core, id: &str) -> bool {
    if !exists(id) || busy(core, id) {
        return false;
    }
    remove(core, id);
    util::log(&format!("projects: deleted {id}"));
    changed(core);
    true
}

// ---------------------------------------------------------------------------
// Adding clips

/// Library clips `ids` (in that order, a repeat counted once) for a project
/// that already has `have` clips and `have_secs` seconds.
fn pick(core: &Core, ids: &[String], have: usize, have_secs: f64) -> Result<Vec<ClipMeta>, String> {
    let mut uniq: Vec<&String> = Vec::new();
    for id in ids {
        if !uniq.contains(&id) {
            uniq.push(id);
        }
    }
    if uniq.is_empty() {
        return Err(NOTHING.into());
    }
    if have + uniq.len() > MAX_CLIPS {
        return Err(TOO_MANY.into());
    }
    let clips: Vec<ClipMeta> = {
        let lib = core.lib.lock().unwrap();
        uniq.iter().map(|id| lib.get(id).filter(|c| !c.draft).ok_or_else(|| "Clip not found".to_string())).collect::<Result<_, _>>()?
    };
    let secs: f64 = clips.iter().map(|c| c.duration.max(0.0)).sum();
    if have_secs + secs > MAX_TOTAL_SECS {
        return Err(TOO_LONG.into());
    }
    for c in &clips {
        if !Path::new(&c.path).is_file() {
            return Err(format!("The video file of \"{}\" is missing", c.title));
        }
    }
    Ok(clips)
}

/// The project's draft, when its file is still there.
fn draft_of(core: &Core, id: &str) -> Option<ClipMeta> {
    core.lib.lock().unwrap().get(id).filter(|c| c.draft && Path::new(&c.path).is_file())
}

/// What `project_add_clips` answers: the job of a project made before 0.1.38
/// (its draft is rebuilt, "project-changed" follows), or at once for a virtual
/// project: { project, added, starts, titles, job_id: "", mode: "reference", secs }
/// (`starts` = where each added clip starts in project time).
#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum Added {
    Job(Job),
    Now(Value),
}

/// `project_add_clips`: a virtual project (or an empty one of before 0.1.38, which
/// becomes virtual) takes the clips at once; one with a draft queues the old job.
pub fn add(core: &Arc<Core>, id: &str, ids: Vec<String>) -> Result<Added, String> {
    let proj = stored(id).ok_or(NOT_FOUND)?;
    let empty_old = !proj.virt && proj.clips.is_empty() && draft_of(core, id).is_none() && pending_of(core, id).is_empty();
    if proj.virt || empty_old {
        return add_now(core, id, ids).map(Added::Now);
    }
    // F1 (0.1.41): an old project with its draft becomes one of references at its first add: no job, no join.
    if let (Some(d), true) = (draft_of(core, id), pending_of(core, id).is_empty()) {
        // The refusals first, so a refused add leaves it as it was.
        pick(core, &ids, proj.clips.len(), d.duration.max(0.0))?;
        to_references(id, &d);
        return add_now(core, id, ids).map(Added::Now);
    }
    add_clips(core, id, ids).map(Added::Job)
}

/// F1: project `id` (made before 0.1.38) keeps its draft `d` as its first source, exactly as it is, and is a
/// project of references from now on. Its clips so far are the ones inside the draft.
fn to_references(id: &str, d: &ClipMeta) {
    let names: Vec<String> = (0..=d.joins.len()).map(|i| d.join_names.get(i).cloned().unwrap_or_default()).collect();
    let base = Base { duration: d.duration.max(0.0), joins: d.joins.clone(), names, width: d.width, height: d.height, fps: d.fps, audio_tracks: d.audio_tracks, size: d.size };
    edit(id, |p| {
        if p.virt {
            return;
        }
        p.base_clips = p.clips.len();
        p.base = Some(base);
        p.virt = true;
    });
    util::log(&format!("projects: {id} made before 0.1.38 now references its clips; its draft ({:.1} s, {} parts) is its first source", d.duration, d.joins.len() + 1));
}

/// Append library clips `ids` to project `pid` as references: no job, no file.
/// A repeat inside one call counts once; a clip already in the project may come
/// again (it is then two sources).
fn add_now(core: &Core, pid: &str, ids: Vec<String>) -> Result<Value, String> {
    let proj = stored(pid).ok_or(NOT_FOUND)?;
    let clips = pick(core, &ids, proj.clips.len(), proj_len(&proj))?;
    let added: Vec<ProjectClip> = clips.iter().map(pclip).collect();
    let titles: Vec<String> = added.iter().map(|c| c.title.clone()).collect();
    // Checked again under the store's lock, against the list as it is now.
    let starts = edit(pid, |p| {
        if p.clips.len() + added.len() > MAX_CLIPS {
            return Err(TOO_MANY.to_string());
        }
        let mut at: f64 = proj_len(p); // after the draft source of an old project (F1), else the clips' spans
        if at + added.iter().map(span).sum::<f64>() > MAX_TOTAL_SECS {
            return Err(TOO_LONG.to_string());
        }
        let mut starts = Vec::with_capacity(added.len());
        for c in &added {
            starts.push(at);
            at += span(c);
        }
        p.virt = true;
        p.clips.extend(added.iter().cloned());
        p.modified = util::unix_ms().max(p.modified + 1);
        p.exported = false;
        Ok(starts)
    })
    .ok_or(NOT_FOUND)??;
    util::log(&format!("projects: {pid}: {} clip(s) added as references at {starts:?}", added.len()));
    changed(core);
    Ok(json!({ "project": get(core, pid), "added": added, "starts": starts, "titles": titles, "job_id": "", "mode": "reference", "secs": 0.0 }))
}

/// `--speed-test` (speedtest.rs): an old project (a draft made by a job) of clips `first`, then clips `more`
/// added the way the UI adds them (`add`), timed until they are in (ms).
pub(crate) fn legacy_add_ms(core: &Arc<Core>, first: &[String], more: &[String]) -> Result<f64, String> {
    let wait = |jid: &str| -> Result<(), String> {
        let until = Instant::now() + std::time::Duration::from_secs(600);
        loop {
            let j = core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();
            if !matches!(j.state.as_str(), "queued" | "running") {
                return if j.state == "done" { Ok(()) } else { Err(format!("{} {}", j.state, j.message)) };
            }
            if Instant::now() > until {
                return Err("timed out".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    let p = make("Old one (speed test)", false);
    let j = enqueue_add(core, &p.id, first.to_vec(), false)?;
    wait(&j.id)?;
    let t = Instant::now();
    if let Added::Job(j) = add(core, &p.id, more.to_vec())? {
        wait(&j.id)?;
    }
    Ok(t.elapsed().as_secs_f64() * 1000.0)
}

/// Queue a job that appends library clips `ids` to project `id` (a project made before 0.1.38).
pub fn add_clips(core: &Arc<Core>, id: &str, ids: Vec<String>) -> Result<Job, String> {
    enqueue_add(core, id, ids, false)
}

/// "Edit together" from the library (0.1.38): a virtual project of the clips, in
/// the order given, named as `joined_title` does, made at once (the refusals and
/// texts of join.rs). The UI opens it.
pub fn join_clips(core: &Core, ids: Vec<String>) -> Result<Project, String> {
    // The project's own limits (0.1.41), not the old joining ones of `join::pick`.
    let mut uniq: Vec<&String> = Vec::new();
    for id in &ids {
        if !uniq.contains(&id) {
            uniq.push(id);
        }
    }
    if uniq.len() < 2 {
        return Err("Pick at least two clips to edit together".into());
    }
    let clips = pick(core, &ids, 0, 0.0)?;
    let p = make(&join::joined_title(&clips[0].title, clips.len() - 1), true);
    let list: Vec<ProjectClip> = clips.iter().map(pclip).collect();
    edit(&p.id, |s| s.clips = list);
    util::log(&format!("projects: Edit together made {} of {} clip(s)", p.id, clips.len()));
    changed(core);
    get(core, &p.id).ok_or_else(|| NOT_FOUND.into())
}

/// "Edit together" as it was before 0.1.38 (a project with a joined draft, made by
/// a job). Only the self-tests still make such projects (`join::enqueue`), to prove they keep working.
pub(crate) fn join_clips_legacy(core: &Arc<Core>, ids: Vec<String>) -> Result<Job, String> {
    let clips = join::pick(core, &ids)?;
    let ids: Vec<String> = clips.iter().map(|c| c.id.clone()).collect();
    let key = format!("join:{}", ids.join(","));
    if core.jobs.list().iter().any(|j| j.clip_id == key && matches!(j.state.as_str(), "queued" | "running")) {
        return Err("Those clips are already being joined".into());
    }
    let p = make(&join::joined_title(&clips[0].title, clips.len() - 1), false);
    match enqueue_add(core, &p.id, ids, true) {
        Ok(job) => Ok(job),
        Err(e) => {
            remove(core, &p.id);
            Err(e)
        }
    }
}

/// Held by an add job's runner. Dropped without the runner having run (cancelled
/// while queued, or Glint closing): forget the pending add, remove a project of
/// "Edit together" that never got its draft, and tell the UI.
struct NotRun {
    core: std::sync::Weak<Core>,
    pid: String,
    token: u64,
    from_join: bool,
    armed: bool,
}

impl Drop for NotRun {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        unpend(self.token);
        // Dropped while the job list is locked: do the rest on another thread (it takes the library's lock).
        let (weak, pid, from_join) = (self.core.clone(), self.pid.clone(), self.from_join);
        std::thread::spawn(move || {
            if let Some(core) = weak.upgrade() {
                if from_join && draft_of(&core, &pid).is_none() {
                    remove(&core, &pid);
                }
                changed(&core);
            }
        });
    }
}

fn enqueue_add(core: &Arc<Core>, pid: &str, ids: Vec<String>, from_join: bool) -> Result<Job, String> {
    let proj = stored(pid).ok_or(NOT_FOUND)?;
    let draft = draft_of(core, pid);
    let (mut have, mut have_secs) = match &draft {
        Some(d) => (proj.clips.len(), d.duration.max(0.0)),
        None => (0, 0.0),
    };
    for (n, secs) in pending_of(core, pid) {
        have += n;
        have_secs += secs;
    }
    let clips = pick(core, &ids, have, have_secs)?;
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    // The job reads the draft (if any) and every clip: `Job::uses` sees each, so none is renamed or moved under it.
    let mut srcs: Vec<&str> = Vec::new();
    if draft.is_some() {
        srcs.push(pid);
    }
    srcs.extend(clips.iter().map(|c| c.id.as_str()));
    let key = format!("join:{}", srcs.join(","));
    if core.jobs.list().iter().any(|j| j.clip_id == key && matches!(j.state.as_str(), "queued" | "running")) {
        return Err("Those clips are already being joined".into());
    }
    let token = TOKEN.fetch_add(1, Ordering::Relaxed);
    let secs: f64 = clips.iter().map(|c| c.duration.max(0.0)).sum();
    PENDING.lock().unwrap().push(Pending { token, project: pid.to_string(), job_id: String::new(), clips: clips.len(), secs });
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let pid_run = pid.to_string();
    let key_run = key.clone();
    let job_id: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let job_id_run = job_id.clone();
    // A job cancelled while it waits is dropped without running: the guard then tidies up.
    let guard = NotRun { core: weak.clone(), pid: pid.to_string(), token, from_join, armed: true };
    let run: Runner = Box::new(move |cancel, progress| {
        let mut guard = guard;
        guard.armed = false;
        let Some(core) = weak.upgrade() else {
            unpend(token);
            return Err("Glint is closing".into());
        };
        let jid = {
            let j = job_id_run.lock().unwrap().clone();
            if j.is_empty() { core.jobs.list().into_iter().rev().find(|x| x.clip_id == key_run && x.state == "running").map(|x| x.id).unwrap_or_default() } else { j }
        };
        let res = build(&core, &pid_run, &clips, &ffmpeg, &s, &jid, cancel, progress);
        unpend(token);
        if let Err(e) = &res {
            util::log(&format!("projects: adding to {pid_run} failed: {e}"));
            // "Edit together" whose first build failed: no empty project is left behind.
            if from_join && draft_of(&core, &pid_run).is_none() {
                remove(&core, &pid_run);
            }
        }
        changed(&core);
        res
    });
    match core.jobs.enqueue(&key, &proj.name, "join", run) {
        Ok(job) => {
            *job_id.lock().unwrap() = job.id.clone();
            if let Some(p) = PENDING.lock().unwrap().iter_mut().find(|x| x.token == token) {
                p.job_id = job.id.clone();
            }
            changed(core);
            Ok(job)
        }
        Err(e) => {
            unpend(token);
            Err(e)
        }
    }
}

/// A file name in `dir` that is free: `<id>.mp4` the first time, `<id>-<new id>.mp4` after.
fn draft_path(dir: &Path, id: &str, first: bool) -> PathBuf {
    let plain = dir.join(format!("{id}.mp4"));
    if first && !plain.exists() {
        return plain;
    }
    loop {
        let p = dir.join(format!("{id}-{}.mp4", util::new_id()));
        if !p.exists() {
            return p;
        }
    }
}

/// Copy `from` to `to` as "Edit together" of one clip: written as `.<name>.joining` first.
fn copy_one(from: &Path, to: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let part = to.with_file_name(format!(".{name}.joining"));
    let _ = fs::remove_file(&part);
    let r = fs::copy(from, &part).map_err(|e| format!("Could not copy the clip: {e}"));
    if r.is_err() || cancel.load(Ordering::SeqCst) {
        let _ = fs::remove_file(&part);
        return Err(if cancel.load(Ordering::SeqCst) { join::CANCELLED.into() } else { r.err().unwrap_or_default() });
    }
    if fs::rename(&part, to).is_err() {
        let _ = fs::remove_file(&part);
        return Err("Could not save the joined clip".into());
    }
    Ok(())
}

/// The parts of a rebuilt draft: the old `joins` and names exactly as they were
/// (names padded to `joins.len() + 1` with "Clip <n>"), then one part per new
/// clip. `starts[0]` is where the old draft starts in the new file (unused),
/// `starts[k]` where new clip k-1 starts; `titles` their titles. A start that
/// is not 0.1 s after the part before it, or too near the end, is dropped as
/// `clean_parts` does: the part before it (never an old one) takes its name;
/// the first new clip too near the old last part simply runs on in that part.
pub fn append_parts(joins: &[f64], names: &[String], starts: &[f64], titles: &[String], length: f64) -> (Vec<f64>, Vec<String>) {
    let mut out_j: Vec<f64> = joins.to_vec();
    let mut out_n: Vec<String> = (0..=joins.len()).map(|i| names.get(i).filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("Clip {}", i + 1))).collect();
    let fixed = out_n.len();
    for (k, &t) in starts.iter().enumerate().skip(1) {
        let name = titles.get(k - 1).filter(|n| !n.trim().is_empty()).cloned().unwrap_or_else(|| format!("Clip {}", fixed + k));
        let t = (t * 1000.0).round() / 1000.0;
        if length > 0.0 && t >= length - 0.1 {
            continue;
        }
        if t > 0.1 && out_j.last().is_none_or(|&l| t > l + 0.1) {
            out_j.push(t);
            out_n.push(name);
        } else if out_n.len() > fixed {
            *out_n.last_mut().unwrap() = name;
        }
    }
    (out_j, out_n)
}

/// The job: build the project's new draft and register it.
#[allow(clippy::too_many_arguments)]
fn build(core: &Arc<Core>, pid: &str, clips: &[ClipMeta], ffmpeg: &Path, s: &crate::settings::Settings, job_id: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(f32)) -> Result<String, String> {
    let started = Instant::now();
    let proj = stored(pid).ok_or(NOT_FOUND)?;
    for c in clips {
        if !Path::new(&c.path).is_file() {
            return Err(format!("The video file of \"{}\" is missing", c.title));
        }
    }
    let old = draft_of(core, pid);
    let ffprobe = library::ffprobe_for(ffmpeg).ok_or("Could not read the clip (ffprobe.exe was not found)")?;
    // Every file is read at once (two ffprobe runs each): one after the other took seconds for twenty clips.
    let files: Vec<(&Path, &str)> = old.iter().chain(clips.iter()).map(|c| (Path::new(&c.path), c.title.as_str())).collect();
    let srcs = join::probe_all(&ffprobe, &files).into_iter().collect::<Result<Vec<_>, _>>()?;
    if cancel.load(Ordering::SeqCst) {
        return Err(join::CANCELLED.into());
    }
    progress(1.0);
    // The new draft goes beside the old one, else into `<clips_dir>\.drafts` (beside the first clip without a clips folder).
    let dir = match &old {
        Some(d) => Path::new(&d.path).parent().map(Path::to_path_buf).unwrap_or_default(),
        None => {
            let root = if s.clips_dir.trim().is_empty() { Path::new(&clips[0].path).parent().map(Path::to_path_buf).unwrap_or_default() } else { PathBuf::from(&s.clips_dir) };
            library::drafts_dir(&root)
        }
    };
    let out = draft_path(&dir, pid, old.is_none());
    let mut fps: Vec<u32> = Vec::new();
    if let Some(d) = &old {
        fps.push(d.fps);
    }
    fps.extend(clips.iter().map(|c| c.fps));
    let (mode, starts, duration) = if srcs.len() == 1 {
        copy_one(Path::new(&clips[0].path), &out, cancel)?;
        progress(90.0);
        (Mode::Copy, vec![0.0], clips[0].duration)
    } else {
        // The draft (when there is one) is never re-encoded to match what is added: the clips are matched to it.
        let done = join::join(ffmpeg, &srcs, &fps, old.is_some(), &out, &proj.name, &s.encoder, cancel, progress)?;
        (done.mode, done.starts, done.duration)
    };
    if cancel.load(Ordering::SeqCst) {
        let _ = fs::remove_file(&out);
        return Err(join::CANCELLED.into());
    }
    // The thumbnail is named after the file, so it changes with it too.
    let stem = out.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| pid.to_string());
    let mut new = library::register_id(ffmpeg, &out, stem, &proj.name, "", "joined");
    new.id = pid.to_string();
    new.draft = true;
    let first = old.as_ref().unwrap_or(&clips[0]);
    new.game = first.game.clone();
    new.folder = first.folder.clone();
    new.created = old.as_ref().map(|d| d.created).unwrap_or(new.created);
    let titles: Vec<String> = clips.iter().map(|c| c.title.clone()).collect();
    let length = new.duration.max(duration);
    let (new_starts, new_titles): (Vec<f64>, Vec<String>);
    match &old {
        Some(d) => {
            (new.joins, new.join_names) = append_parts(&d.joins, &d.join_names, &starts, &titles, length);
            // The new parts, for the editor's emAddParts: source start and title of each.
            new_starts = new.joins[d.joins.len()..].to_vec();
            new_titles = new.join_names[d.joins.len() + 1..].to_vec();
        }
        None => {
            (new.joins, new.join_names) = join::clean_parts(&starts, &titles, length);
            new_starts = std::iter::once(0.0).chain(new.joins.iter().copied()).collect();
            new_titles = new.join_names.clone();
        }
    }
    if new.fps == 0 || mode != Mode::Encode {
        new.fps = fps.iter().copied().max().unwrap_or(new.fps).max(new.fps);
    }
    core.lib.lock().unwrap().add(new.clone());
    if let Some(d) = &old {
        if d.path != new.path {
            library::remove_draft_files(d);
        }
    }
    let added: Vec<ProjectClip> = clips.iter().map(pclip).collect();
    let had = old.is_some();
    edit(pid, |p| {
        if !had {
            p.clips.clear();
        }
        p.clips.extend(added.iter().cloned());
        p.modified = util::unix_ms().max(p.modified + 1);
        p.exported = false;
    });
    progress(100.0);
    let secs = started.elapsed().as_secs_f64();
    util::log(&format!("projects: {pid}: {} clip(s) added in {secs:.1} s, mode {}, joins {:?}", clips.len(), mode.name(), new.joins));
    let project = get(core, pid);
    emit(core, "project-changed", json!({ "project": project, "job_id": job_id, "added": added, "mode": mode.name(), "secs": secs, "starts": new_starts, "titles": new_titles }));
    // "clip-joined" is no longer sent: the frontend listens to "project-changed" only (0.1.16).
    Ok(new.id)
}

// ---------------------------------------------------------------------------
// Start-up

/// Keep the drafts of projects; remove every other draft and file in `.drafts`
/// (library::sweep_drafts) and every state file that belongs to no project.
pub fn sweep_at_start(lib: &mut Library, clips_dir: &Path) -> usize {
    let keep = ids();
    let n = library::sweep_drafts(lib, clips_dir, &keep);
    if let Ok(rd) = fs::read_dir(states_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours = name.strip_suffix(".state.json").is_some_and(|id| keep.iter().any(|k| k == id));
            if !ours && e.path().is_file() && (name.ends_with(".state.json") || name.ends_with(".state.tmp")) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    n
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

/// `glint.exe --project-test <report.txt>`: headless checks of editor projects.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0, passed: 0 };
    let _ = fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run_self_test(&mut r);
    store_safety_checks(&mut r);
    let (passed, failed) = (r.passed, r.failed);
    r.line(&format!("DONE passed={passed} failed={failed}"));
}

/// 0.1.41, R3-9: a projects.json that cannot be read is never saved over (and the last good one is kept as .bak).
fn store_safety_checks(r: &mut Report) {
    let path = file();
    let had = fs::read(&path).ok();
    let garbage = b"{ this is not the list Glint writes".to_vec();
    let _ = fs::write(&path, &garbage);
    test_reset_unreadable();
    let seen = load();
    let p = make("Made while the file was unreadable", true);
    let kept = fs::read(&path).ok() == Some(garbage.clone());
    r.check("projects: an unreadable projects.json reads as empty and is never saved over", seen.is_empty() && kept, &p.id);
    // Back to the file the tests had, and a save that keeps the last good one as .bak.
    test_reset_unreadable();
    match had {
        Some(b) => {
            let _ = fs::write(&path, &b);
            let list = load();
            save(&list);
            let bak = fs::read(path.with_extension("json.bak")).ok();
            r.check("projects: a save keeps the last good file as projects.json.bak", bak.as_deref() == Some(b.as_slice()), "");
        }
        None => {
            let _ = fs::remove_file(&path);
        }
    }
}
fn wait_until(secs: f64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + std::time::Duration::from_secs_f64(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    cond()
}

fn events() -> Vec<(String, Value)> {
    RECORD.lock().unwrap().clone().unwrap_or_default()
}

/// A project as 0.1.37 made them (a draft file, filled by jobs), to prove those keep working.
fn create_legacy(core: &Core, name: &str) -> Project {
    let p = make(name, false);
    changed(core);
    get(core, &p.id).unwrap_or_default()
}

fn run_self_test(r: &mut Report) {
    // 0. Pure: appending parts keeps the old ones exactly.
    let sv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let (j, n) = append_parts(&[4.0, 7.25], &sv(&["A", "B", "C"]), &[0.02, 10.0334, 13.5], &sv(&["D", "E"]), 16.0);
    r.check("append_parts: old joins and names kept, new ones appended (rounded)", j == [4.0, 7.25, 10.033, 13.5] && n == sv(&["A", "B", "C", "D", "E"]), &format!("{j:?} {n:?}"));
    let (j, n) = append_parts(&[4.0], &sv(&["A"]), &[0.0, 4.05, 9.0, 9.05, 15.95], &sv(&["C", "D", "E", "F"]), 16.0);
    r.check("append_parts: missing old name padded, too-close or too-late starts dropped, an old part never renamed", j == [4.0, 9.0] && n == sv(&["A", "Clip 2", "E"]), &format!("{j:?} {n:?}"));
    let (j, n) = append_parts(&[], &[], &[0.0, 5.0], &sv(&["B"]), 8.0);
    r.check("append_parts: onto a one-clip draft", j == [5.0] && n == sv(&["Clip 1", "B"]), &format!("{j:?} {n:?}"));
    r.check("state file: only a plain id", state_file("ab12-x_").is_some() && state_file("..\\x").is_none() && state_file("").is_none() && state_file("a/b").is_none(), "");

    let Some(ffmpeg) = util::find_exe("", "ffmpeg") else {
        r.check("ffmpeg found", false, "");
        return;
    };
    let data = util::data_dir();
    let _ = fs::remove_file(file());
    let _ = fs::remove_dir_all(states_dir());
    let _ = fs::remove_file(data.join("library.json"));
    let root = data.join("project clips");
    let _ = fs::remove_dir_all(&root);
    let dir = root.join("Proj Game");
    let _ = fs::create_dir_all(&dir);
    *RECORD.lock().unwrap() = Some(Vec::new());

    // Three clips of the same format (a straight copy joins them), 3 named tracks each.
    let core = Core::new();
    {
        let mut s = core.settings.lock().unwrap();
        s.clips_dir = root.to_string_lossy().into_owned();
        s.encoder = "x264".into();
    }
    core.jobs.connect(&core);
    let mut ids: Vec<String> = Vec::new();
    for (name, secs, flash) in [("pa", 4.0, 1.0), ("pb", 3.0, 1.5), ("pc", 3.0, 2.0), ("px", 2.0, 1.0)] {
        let p = dir.join(format!("{name}.mp4"));
        let sp = join::Spec { w: 320, h: 240, fps: 30, secs, audio_extra: 0.0, tracks: 3, vfr: false, sar: "1", flash, names: &[] };
        if let Err(e) = join::make_clip(&ffmpeg, &p, &sp) {
            r.check(&format!("test clip {name} made"), false, &e);
            return;
        }
        let mut m = library::register(&ffmpeg, &p, &format!("Clip {}", name.to_uppercase()), "Proj Game", "clip");
        m.folder = "Proj Game".into();
        m.fps = 30;
        ids.push(m.id.clone());
        core.lib.lock().unwrap().add(m);
    }
    let (a, b, c, x) = (ids[0].clone(), ids[1].clone(), ids[2].clone(), ids[3].clone());
    let job_state = |jid: &str| core.jobs.list().into_iter().find(|j| j.id == jid).unwrap_or_default();
    let finish = |job: &Result<Job, String>| -> Job {
        match job {
            Ok(j) => {
                wait_until(120.0, || !matches!(job_state(&j.id).state.as_str(), "queued" | "running"));
                job_state(&j.id)
            }
            Err(_) => Job::default(),
        }
    };
    let hold = || {
        core.jobs.enqueue("join:hold", "hold", "join", Box::new(|cancel, _| {
            while !cancel.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(join::CANCELLED.into())
        }))
    };
    let release = |job: Result<Job, String>| {
        if let Ok(j) = job {
            core.jobs.cancel(&j.id);
            wait_until(10.0, || !matches!(job_state(&j.id).state.as_str(), "queued" | "running"));
        }
    };

    // 1-7 run on projects as 0.1.37 made them (a draft file, jobs): they must keep working.
    // 1. Create, list, refusals.
    let p1 = create_legacy(&core, "  ");
    r.check("create: empty, named \"New project\", no draft, not busy", p1.name == DEFAULT_NAME && p1.clip.is_none() && p1.clips.is_empty() && !p1.busy && p1.size == 0 && !p1.exported && p1.created > 0 && p1.modified == p1.created, &format!("{p1:?}"));
    r.check("create: \"projects-changed\" sent", events().iter().any(|(e, v)| e == "projects-changed" && v.is_null()), "");
    r.check("list: has it", list(&core).iter().any(|p| p.id == p1.id), "");
    let saved = fs::read_to_string(file()).unwrap_or_default();
    r.check("projects.json keeps it", saved.contains(&p1.id) && saved.contains(DEFAULT_NAME), "");
    r.check("add: nothing is refused", add_clips(&core, &p1.id, vec![]).is_err_and(|e| e == NOTHING), "");
    r.check("add: unknown project is refused", add_clips(&core, "nope", vec![a.clone()]).is_err_and(|e| e == NOT_FOUND), "");
    let many: Vec<String> = (0..MAX_CLIPS + 1).map(|i| format!("z{i}")).collect();
    r.check("add: more clips than the limit are refused", add_clips(&core, &p1.id, many).is_err_and(|e| e == TOO_MANY), "");
    r.check("add: an unknown clip is refused", add_clips(&core, &p1.id, vec!["zz".into()]).is_err_and(|e| e == "Clip not found"), "");
    {
        let mut lib = core.lib.lock().unwrap();
        lib.add(ClipMeta { id: "long".into(), path: dir.join("pa.mp4").to_string_lossy().into_owned(), title: "long".into(), duration: MAX_TOTAL_SECS + 1.0, ..Default::default() });
    }
    r.check("add: over the time limit is refused", add_clips(&core, &p1.id, vec!["long".into()]).is_err_and(|e| e == TOO_LONG), "");
    core.lib.lock().unwrap().remove("long");
    r.check("refusals queued nothing and left the project as it was", core.jobs.list().is_empty() && get(&core, &p1.id).is_some_and(|p| p.clip.is_none() && !p.busy), "");

    // 2. One clip: the draft is that clip, lossless.
    let j = add_clips(&core, &p1.id, vec![a.clone()]);
    r.check("add one clip: queued as a \"join\" job titled after the project", j.as_ref().is_ok_and(|j| j.kind == "join" && j.title == DEFAULT_NAME), &format!("{:?}", j.as_ref().err()));
    r.check("add: the project is busy while its job waits or runs", get(&core, &p1.id).is_some_and(|p| p.busy), "");
    r.check("delete is refused while busy", !delete(&core, &p1.id) && exists(&p1.id), "");
    let done = finish(&j);
    r.check("add one clip: job done, result is the project id", done.state == "done" && done.result_id == p1.id, &format!("{} {}", done.state, done.message));
    let one = get(&core, &p1.id).unwrap_or_default();
    let d1 = one.clip.clone().unwrap_or_default();
    let pa = dir.join("pa.mp4");
    r.check("one clip: draft is <clips>\\.drafts\\<id>.mp4, kind joined, draft, id = project id", Path::new(&d1.path) == root.join(".drafts").join(format!("{}.mp4", p1.id)) && d1.kind == "joined" && d1.draft && d1.id == p1.id, &d1.path);
    r.check("one clip: a byte-for-byte copy (lossless)", fs::read(&d1.path).ok() == fs::read(&pa).ok(), "");
    r.check("one clip: no joins, one name (the clip's title)", d1.joins.is_empty() && d1.join_names == ["Clip PA"], &format!("{:?} {:?}", d1.joins, d1.join_names));
    r.check("one clip: clips = [a], size = the file, not busy", one.clips.len() == 1 && one.clips[0].id == a && one.clips[0].title == "Clip PA" && one.size == fs::metadata(&d1.path).map(|m| m.len()).unwrap_or(0) && one.size > 0 && !one.busy, &format!("{:?}", one.clips));
    r.check("one clip: the draft is not in list_clips", !core.lib.lock().unwrap().visible().iter().any(|c| c.id == p1.id), "");
    r.check("one clip: game and folder of the clip, thumbnail made", d1.game == "Proj Game" && d1.folder == "Proj Game" && Path::new(&d1.thumb).is_file(), &d1.thumb);
    let ev = events().into_iter().rev().find(|(e, _)| e == "project-changed").map(|(_, v)| v).unwrap_or_default();
    r.check(
        "project-changed: project, job_id, added, mode, secs, starts [0], titles",
        ev["project"]["id"] == p1.id.as_str() && ev["job_id"] == done.id.as_str() && ev["added"].as_array().is_some_and(|x| x.len() == 1 && x[0]["id"] == a.as_str()) && ev["mode"] == "copy" && ev["secs"].is_number() && ev["starts"] == json!([0.0]) && ev["titles"] == json!(["Clip PA"]),
        &ev.to_string(),
    );
    r.check("clip-joined is no longer sent", !events().iter().any(|(e, _)| e == "clip-joined"), "");

    // 3. Add two more: old joins kept, new appended, the path and thumbnail change.
    let j = add_clips(&core, &p1.id, vec![b.clone(), c.clone(), b.clone()]);
    r.check("add to a draft: the job key lists the draft, then the clips", j.as_ref().is_ok_and(|j| j.clip_id == format!("join:{},{},{}", p1.id, b, c)), &format!("{:?}", j.as_ref().map(|j| j.clip_id.clone())));
    let done = finish(&j);
    r.check("add to a draft: job done", done.state == "done", &done.message);
    let two = get(&core, &p1.id).unwrap_or_default();
    let d2 = two.clip.clone().unwrap_or_default();
    r.check("add to a draft: new file in .drafts, old file and thumbnail gone, new thumbnail", d2.path != d1.path && Path::new(&d2.path).is_file() && !Path::new(&d1.path).exists() && d2.thumb != d1.thumb && Path::new(&d2.thumb).is_file() && !Path::new(&d1.thumb).exists() && library::in_drafts_dir(Path::new(&d2.path)), &format!("{} / {}", d1.path, d2.path));
    r.check("add to a draft: still the project id, a draft, not visible", d2.id == p1.id && d2.draft && !core.lib.lock().unwrap().visible().iter().any(|c| c.id == p1.id), "");
    r.check("add to a draft: joins at about 4 s and 7 s, names in order", d2.joins.len() == 2 && (4.0..4.15).contains(&d2.joins[0]) && (7.0..7.25).contains(&d2.joins[1]) && d2.join_names == ["Clip PA", "Clip PB", "Clip PC"], &format!("{:?} {:?}", d2.joins, d2.join_names));
    r.check("add to a draft: length about 10 s", (d2.duration - 10.0).abs() < 0.2, &format!("{:.3}", d2.duration));
    r.check("add to a draft: clips = [a, b, c] (a repeat in one add counts once)", two.clips.iter().map(|x| x.id.clone()).collect::<Vec<_>>() == [a.clone(), b.clone(), c.clone()], "");
    r.check("add to a draft: modified moved on, not exported", two.modified > one.modified && !two.exported, "");
    let ev = events().into_iter().rev().find(|(e, _)| e == "project-changed").map(|(_, v)| v).unwrap_or_default();
    let st: Vec<f64> = ev["starts"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    r.check("project-changed: starts and titles of the NEW parts only", st == d2.joins && ev["titles"] == json!(["Clip PB", "Clip PC"]) && ev["added"].as_array().is_some_and(|x| x.len() == 2), &ev.to_string());

    // 4. "Edit together" makes a project; adding again keeps the old joins exactly.
    let j = join_clips_legacy(&core, vec![b.clone(), a.clone()]);
    let done = finish(&j);
    let p2 = get(&core, &done.result_id).unwrap_or_default();
    let e1 = p2.clip.clone().unwrap_or_default();
    r.check("join_clips: makes a project named \"<first> + 1 more\" with the draft", done.state == "done" && p2.name == "Clip PB + 1 more" && e1.title == p2.name && e1.joins.len() == 1 && e1.join_names == ["Clip PB", "Clip PA"], &format!("{} {:?} {:?}", p2.name, e1.joins, e1.join_names));
    r.check("join_clips: one clip is still refused, as before", join_clips_legacy(&core, vec![a.clone()]).is_err_and(|e| e == "Pick at least two clips to edit together"), "");
    let j = add_clips(&core, &p2.id, vec![c.clone()]);
    let done = finish(&j);
    let e2 = get(&core, &p2.id).and_then(|p| p.clip).unwrap_or_default();
    r.check("add again: the old joins are unchanged, the new one appended", done.state == "done" && e2.joins.len() == 2 && e2.joins[0] == e1.joins[0] && e2.joins[1] > e1.joins[0] + 2.5 && e2.join_names == ["Clip PB", "Clip PA", "Clip PC"], &format!("{:?} -> {:?}", e1.joins, e2.joins));
    r.check("list: most recently modified first", list(&core).first().map(|p| p.id.clone()) == Some(p2.id.clone()), "");

    // 5. A failed add leaves the project as it was; a failed first "Edit together" leaves no project.
    let before = get(&core, &p1.id).unwrap_or_default();
    let blocker = hold();
    let px = dir.join("px.mp4");
    let j = add_clips(&core, &p1.id, vec![x.clone()]);
    let n_before = list(&core).len();
    let jt = join_clips_legacy(&core, vec![x.clone(), a.clone()]);
    r.check("adds wait behind another join job, busy", j.is_ok() && jt.is_ok() && get(&core, &p1.id).is_some_and(|p| p.busy) && list(&core).len() == n_before + 1, &format!("{:?} {:?}", j.as_ref().err(), jt.as_ref().err()));
    let _ = fs::rename(&px, dir.join("px gone.mp4"));
    release(blocker);
    let done = finish(&j);
    let donet = finish(&jt);
    let after = get(&core, &p1.id).unwrap_or_default();
    let (b1, a1) = (before.clip.clone().unwrap_or_default(), after.clip.clone().unwrap_or_default());
    r.check("failed add: the job fails, the project is unchanged", done.state == "error" && a1.path == b1.path && a1.joins == b1.joins && after.clips == before.clips && after.modified == before.modified && !after.busy, &format!("{} {}", done.state, done.message));
    r.check("failed first Edit together: its project is removed again", donet.state == "error" && list(&core).len() == n_before && !exists(&donet.result_id), &format!("{} {}", donet.state, donet.message));
    let _ = fs::rename(dir.join("px gone.mp4"), &px);
    let blocker = hold();
    let n0 = list(&core).len();
    let jq = join_clips_legacy(&core, vec![c.clone(), b.clone()]);
    let grew = list(&core).len() == n0 + 1;
    if let Ok(j) = &jq {
        core.jobs.cancel(&j.id);
    }
    let gone = wait_until(5.0, || list(&core).len() == n0);
    r.check("Edit together cancelled while it waits: its project is removed again", jq.is_ok() && grew && gone, &format!("{:?}", jq.as_ref().err()));
    release(blocker);

    // 6. Saved state, export mark, rename.
    r.check("state: nothing saved yet is null", state(&p1.id).is_none(), "");
    let text = "{\"cuts\":[1.5,2],\"name\":\"ü 한\"}\n";
    let m0 = get(&core, &p1.id).map(|p| p.modified).unwrap_or(0);
    mark_exported(&core, &p1.id);
    r.check("export from the draft marks the project exported", get(&core, &p1.id).is_some_and(|p| p.exported), "");
    let s1 = save_state(&core, &p1.id, text);
    r.check("state: round trip, modified set, exported cleared", s1.as_ref().is_ok_and(|p| p.modified >= m0 && !p.exported) && state(&p1.id).as_deref() == Some(text), &format!("{:?}", s1.as_ref().err()));
    r.check("state: unknown project refused", save_state(&core, "nope", "x").is_err() && save_state(&core, "..\\evil", "x").is_err() && state("..\\evil").is_none(), "");
    let rn = rename(&core, &p1.id, "  My montage ");
    r.check("rename: the name and the draft's title", rn.as_ref().is_ok_and(|p| p.name == "My montage" && p.clip.as_ref().is_some_and(|c| c.title == "My montage")), &format!("{:?}", rn.as_ref().err()));
    r.check("rename: a blank name is refused", rename(&core, &p1.id, "  ").is_err_and(|e| e == NO_NAME), "");

    // 7. Delete; discard_draft is another name for it.
    let p2c = get(&core, &p2.id).and_then(|p| p.clip).unwrap_or_default();
    let _ = save_state(&core, &p2.id, "x");
    r.check("delete: true, draft entry, file, thumbnail and state gone", delete(&core, &p2.id) && !exists(&p2.id) && core.lib.lock().unwrap().get(&p2.id).is_none() && !Path::new(&p2c.path).exists() && !Path::new(&p2c.thumb).exists() && state(&p2.id).is_none(), "");
    r.check("delete: again is false", !delete(&core, &p2.id), "");
    let p3 = create_legacy(&core, "Third");
    let j = add_clips(&core, &p3.id, vec![c.clone()]);
    let _ = finish(&j);
    let p3c = get(&core, &p3.id).and_then(|p| p.clip).unwrap_or_default();
    r.check("discard_draft deletes a project too", join::discard_draft(&core, &p3.id) && !exists(&p3.id) && !Path::new(&p3c.path).exists(), "");
    r.check("the source clips are untouched", [&a, &b, &c].iter().all(|id| core.lib.lock().unwrap().get(id).is_some_and(|m| Path::new(&m.path).is_file())), "");

    // 7b. 0.1.38: projects of references (no draft file, no job).
    let drafts_dir = root.join(".drafts");
    let n_files = || fs::read_dir(&drafts_dir).map(|r| r.count()).unwrap_or(0);
    let meta = |id: &str| core.lib.lock().unwrap().get(id).unwrap_or_default();
    let (ma, mb, mx) = (meta(&a), meta(&b), meta(&x));
    let v1 = create(&core, "Refs");
    r.check("virtual: create makes a project of references, empty, no clip", v1.virt && v1.clip.is_none() && v1.clip_json.is_none() && v1.clips.is_empty() && v1.name == "Refs", &format!("{v1:?}"));
    r.check("virtual: project_open of an empty one is refused", open(&core, &v1.id).is_err_and(|e| e == EMPTY), "");
    let files0 = n_files();
    let jobs0 = core.jobs.list().len();
    let pc_n = || events().iter().filter(|(e, _)| e == "project-changed").count();
    let pc0 = pc_n();
    let t0 = Instant::now();
    let ad = add(&core, &v1.id, vec![a.clone(), b.clone(), a.clone()]);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let now = match &ad {
        Ok(Added::Now(v)) => v.clone(),
        _ => Value::Null,
    };
    r.check("virtual add: answered at once (no job, no file, no project-changed)", !now.is_null() && core.jobs.list().len() == jobs0 && n_files() == files0 && pc_n() == pc0 && ms < 1000.0, &format!("{ms:.0} ms {:?}", ad.as_ref().err()));
    r.check("virtual add: the answer has project, added, starts [0, a], titles (a repeat in one add counts once)", now["project"]["id"] == v1.id.as_str() && now["added"].as_array().is_some_and(|x| x.len() == 2) && now["starts"] == json!([0.0, ma.duration]) && now["titles"] == json!(["Clip PA", "Clip PB"]) && now["mode"] == "reference" && now["job_id"] == "", &now.to_string());
    let o = open(&core, &v1.id).unwrap_or_default();
    let src = o["sources"].as_array().cloned().unwrap_or_default();
    r.line(&format!("INFO project_open of two clips: {o}"));
    r.check("project_open: virtual, draft, kind joined, no path, id and title of the project", o["virtual"] == true && o["draft"] == true && o["kind"] == "joined" && o["path"] == "" && o["title"] == "Refs" && o["id"] == v1.id.as_str(), &o.to_string());
    r.check("project_open: duration = the sum, joins = the starts [0, a], names in order", o["duration"].as_f64() == Some(ma.duration + mb.duration) && o["joins"] == json!([0.0, ma.duration]) && o["join_names"] == json!(["Clip PA", "Clip PB"]), &o.to_string());
    r.check(
        "project_open: sources in order with clip id, file, start, length, picture, tracks, not missing",
        src.len() == 2 && src[0]["clip"] == a.as_str() && src[0]["path"] == ma.path.as_str() && src[0]["start"] == 0.0 && src[1]["clip"] == b.as_str() && src[1]["path"] == mb.path.as_str() && src[1]["start"].as_f64() == Some(ma.duration) && src[1]["duration"].as_f64() == Some(mb.duration) && src[0]["width"] == ma.width && src[0]["height"] == ma.height && src[0]["fps"] == 30 && src[1]["audio_tracks"] == mb.audio_tracks && src[0]["thumb"] == ma.thumb.as_str() && src.iter().all(|s| s["missing"] == false && s["title"].is_string()),
        &format!("{src:?}"),
    );
    r.check("project_open: the picture and tracks are the first source's", o["width"] == ma.width && o["height"] == ma.height && o["fps"] == ma.fps && o["audio_tracks"] == ma.audio_tracks && o["thumb"] == ma.thumb.as_str(), "");
    let ad2 = add(&core, &v1.id, vec![a.clone()]);
    let o2 = open(&core, &v1.id).unwrap_or_default();
    let s2 = o2["sources"].as_array().cloned().unwrap_or_default();
    let end2 = ma.duration + mb.duration;
    r.check("virtual add: the same clip again is a second source, after the others", matches!(&ad2, Ok(Added::Now(v)) if v["starts"] == json!([end2])) && s2.len() == 3 && s2[2]["clip"] == a.as_str() && s2[2]["start"].as_f64() == Some(end2) && o2["joins"] == json!([0.0, ma.duration, end2]) && o2["duration"].as_f64() == Some(end2 + ma.duration), &o2.to_string());
    r.check("virtual add: clips kept in order with size, picture and tracks", get(&core, &v1.id).is_some_and(|p| p.clips.iter().map(|c| c.id.clone()).collect::<Vec<_>>() == [a.clone(), b.clone(), a.clone()] && p.clips[1].size == mb.size && p.clips[1].fps == 30 && p.clips[1].width == mb.width && p.clips[1].audio_tracks == mb.audio_tracks && p.clips[1].game == "Proj Game" && !p.busy), "");
    // The export (projexport.rs) lays the sources out exactly as project_open does: same clips, starts, paths.
    let ex = crate::projexport::sources_of(&core, &v1.id).map(|(_, s)| s).unwrap_or_default();
    r.check(
        "export_project's sources are project_open's: clips, starts, lengths, paths, none missing",
        ex.len() == s2.len() && ex.iter().zip(&s2).all(|(e, o)| o["clip"] == e.clip.as_str() && o["start"].as_f64() == Some(e.start) && o["duration"].as_f64() == Some(e.duration) && o["path"] == e.path.as_str() && !e.missing),
        &format!("{:?}", ex.iter().map(|e| (e.clip.clone(), e.start, e.duration)).collect::<Vec<_>>()),
    );
    let fake: Vec<String> = (0..MAX_CLIPS - 2).map(|i| format!("z{i}")).collect();
    r.check("virtual add: past the clip limit is refused", add(&core, &v1.id, fake).is_err_and(|e| e == TOO_MANY), "");
    core.lib.lock().unwrap().add(ClipMeta { id: "long".into(), path: ma.path.clone(), title: "long".into(), duration: MAX_TOTAL_SECS - 5.0, ..Default::default() });
    r.check("virtual add: past the time limit is refused", add(&core, &v1.id, vec!["long".into()]).is_err_and(|e| e == TOO_LONG), "");
    core.lib.lock().unwrap().remove("long");
    r.check("virtual add: an unknown clip and nothing are refused", add(&core, &v1.id, vec!["zz".into()]).is_err_and(|e| e == "Clip not found") && add(&core, &v1.id, vec![]).is_err_and(|e| e == NOTHING), "");
    r.check("virtual add: the refusals left it as it was", open(&core, &v1.id).is_ok_and(|o| o["sources"].as_array().map(|s| s.len()) == Some(3)), "");
    let lj = serde_json::to_value(list(&core)).unwrap_or_default();
    let entry = lj.as_array().and_then(|l| l.iter().find(|p| p["id"] == v1.id.as_str()).cloned()).unwrap_or_default();
    r.check("list_projects: virtual true, clip = what project_open returns, size 0", entry["virtual"] == true && entry["clip"] == open(&core, &v1.id).unwrap_or_default() && entry["size"] == 0 && entry["clips"].as_array().map(|c| c.len()) == Some(3), "");

    // "Edit together": a project of references, made at once.
    let n0 = list(&core).len();
    let jv = join_clips(&core, vec![x.clone(), a.clone()]);
    let v2 = jv.clone().unwrap_or_default();
    r.check(
        "Edit together: a project of references made at once, named \"<first> + 1 more\", no job, no file",
        jv.is_ok() && v2.virt && v2.name == "Clip PX + 1 more" && list(&core).len() == n0 + 1 && v2.clip_json.as_ref().is_some_and(|c| c["sources"].as_array().map(|s| s.len()) == Some(2) && c["title"] == "Clip PX + 1 more" && c["sources"][1]["start"].as_f64() == Some(mx.duration)) && core.jobs.list().len() == jobs0 && n_files() == files0,
        &format!("{:?}", jv.as_ref().err()),
    );
    r.check("Edit together: one clip is still refused", join_clips(&core, vec![a.clone()]).is_err_and(|e| e == "Pick at least two clips to edit together"), "");

    // A missing clip: the file gone, then the clip gone from the library.
    let px_path = PathBuf::from(&mx.path);
    let away = px_path.with_file_name("px away.mp4");
    let _ = fs::rename(&px_path, &away);
    let om = open(&core, &v2.id);
    let sm = om.as_ref().map(|o| o["sources"].clone()).unwrap_or_default();
    r.check(
        "missing file: the project opens, that source missing, no path, its title, length and the starts kept",
        om.is_ok() && sm[0]["missing"] == true && sm[0]["path"] == "" && sm[0]["title"] == "Clip PX" && sm[0]["duration"].as_f64() == Some(mx.duration) && sm[1]["start"].as_f64() == Some(mx.duration) && sm[1]["missing"] == false && om.as_ref().is_ok_and(|o| o["width"] == mx.width && o["duration"].as_f64() == Some(mx.duration + ma.duration)),
        &format!("{om:?}"),
    );
    let _ = fs::rename(&away, &px_path);
    r.check("missing file: back again, the source is found", open(&core, &v2.id).is_ok_and(|o| o["sources"][0]["missing"] == false && o["sources"][0]["path"] == mx.path.as_str()), "");
    let gone = core.lib.lock().unwrap().remove(&x);
    let om = open(&core, &v2.id);
    r.check("clip gone from the library: the project opens and lists, the source missing, shown from what was kept", om.as_ref().is_ok_and(|o| o["sources"][0]["missing"] == true && o["sources"][0]["fps"] == 30 && o["sources"][0]["width"] == mx.width && o["sources"][0]["title"] == "Clip PX" && o["sources"][0]["thumb"] == "") && list(&core).iter().any(|p| p.id == v2.id), &format!("{om:?}"));
    if let Some(m) = gone {
        core.lib.lock().unwrap().add(m);
    }

    // Rename, saved edit, export mark, delete.
    let rv = rename(&core, &v1.id, " Refs 2 ");
    r.check("virtual rename: the name and the opened title", rv.is_ok_and(|p| p.name == "Refs 2") && open(&core, &v1.id).is_ok_and(|o| o["title"] == "Refs 2"), "");
    let sv = save_state(&core, &v1.id, "{\"v\":1}");
    r.check("virtual state: saved and read back, exported cleared", sv.is_ok_and(|p| !p.exported) && state(&v1.id).as_deref() == Some("{\"v\":1}"), "");
    mark_exported(&core, &v1.id);
    r.check("virtual: marked exported", get(&core, &v1.id).is_some_and(|p| p.exported), "");
    let lib_n = core.lib.lock().unwrap().clips.len();
    r.check(
        "virtual delete: record and state gone, the library clips and their files untouched",
        delete(&core, &v1.id) && !exists(&v1.id) && state(&v1.id).is_none() && core.lib.lock().unwrap().clips.len() == lib_n && [&a, &b].iter().all(|id| core.lib.lock().unwrap().get(id).is_some_and(|m| Path::new(&m.path).is_file())),
        "",
    );

    // A project made before 0.1.38 (a draft): every command as before.
    let p5 = create_legacy(&core, "Old one");
    let _ = finish(&enqueue_add(&core, &p5.id, vec![a.clone(), c.clone()], false));
    let d5 = get(&core, &p5.id).and_then(|p| p.clip).unwrap_or_default();
    let o5 = open(&core, &p5.id).unwrap_or_default();
    r.check("old project: project_open gives its draft as before (no virtual, its file)", o5["id"] == p5.id.as_str() && o5.get("virtual").is_none() && o5.get("sources").is_none() && o5["path"] == d5.path.as_str() && o5["draft"] == true && Path::new(&d5.path).is_file() && d5.joins.len() == 1, &o5.to_string());
    let lj5 = serde_json::to_value(get(&core, &p5.id)).unwrap_or_default();
    r.check("old project: listed with virtual false and its draft as clip", lj5["virtual"] == false && lj5["clip"]["path"] == d5.path.as_str() && lj5["clip"]["joins"].as_array().map(|j| j.len()) == Some(1), "");
    let renamed5 = rename(&core, &p5.id, "Old one");
    let state5 = "{\"v\":1,\"saved\":\"by 0.1.40\"}";
    let _ = save_state(&core, &p5.id, state5);
    let bytes5 = fs::read(&d5.path).unwrap_or_default();
    let (jobs5, joined5) = (core.jobs.list().len(), events().iter().filter(|(e, _)| e == "project-changed").count());
    // F1 (0.1.41): its first add makes it a project of references at once, the draft as source 0.
    let t5 = Instant::now();
    let a5 = add(&core, &p5.id, vec![b.clone()]);
    let ms5 = t5.elapsed().as_secs_f64() * 1000.0;
    let now5 = match &a5 {
        Ok(Added::Now(v)) => v.clone(),
        _ => Value::Null,
    };
    r.check("F1: an old project's add is answered at once (no job, no join)", renamed5.is_ok() && !now5.is_null() && core.jobs.list().len() == jobs5 && events().iter().filter(|(e, _)| e == "project-changed").count() == joined5 && ms5 < 1000.0 && now5["starts"] == json!([d5.duration]), &format!("{ms5:.0} ms {a5:?}"));
    r.check("F1: its draft file is untouched (same bytes, same path) and its saved edit too", fs::read(&d5.path).unwrap_or_default() == bytes5 && !bytes5.is_empty() && state(&p5.id).as_deref() == Some(state5) && draft_of(&core, &p5.id).is_some_and(|d| d.path == d5.path), "");
    let v5 = open(&core, &p5.id).unwrap_or_default();
    let s5 = v5["sources"].as_array().cloned().unwrap_or_default();
    let j1 = d5.joins.first().copied().unwrap_or(-1.0);
    r.check(
        "F1: project_open: virtual, source 0 = the draft (clip = the project, its file, start 0, its length, draft, inner = its clips)",
        v5["virtual"] == true && s5.len() == 2 && s5[0]["clip"] == p5.id.as_str() && s5[0]["path"] == d5.path.as_str() && s5[0]["start"] == 0.0 && s5[0]["duration"].as_f64() == Some(d5.duration) && s5[0]["draft"] == true && s5[0]["missing"] == false
            && s5[0]["inner"] == json!([{ "at": 0.0, "title": "Clip PA" }, { "at": j1, "title": "Clip PC" }]) && s5[1]["clip"] == b.as_str() && s5[1]["start"].as_f64() == Some(d5.duration),
        &v5.to_string(),
    );
    r.check("F1: joins / join_names list every border (the draft's inner ones, then each later source's start)", v5["joins"] == json!([0.0, j1, d5.duration]) && v5["join_names"] == json!(["Clip PA", "Clip PC", "Clip PB"]) && v5["duration"].as_f64() == Some(d5.duration + mb.duration), &v5.to_string());
    let a5b = add(&core, &p5.id, vec![x.clone()]);
    r.check("F1: the next add is instant too, after the others", matches!(&a5b, Ok(Added::Now(v)) if v["starts"] == json!([d5.duration + mb.duration])) && get(&core, &p5.id).is_some_and(|p| p.virt && p.clips.len() == 4), &format!("{a5b:?}"));
    let lj5 = serde_json::to_value(get(&core, &p5.id)).unwrap_or_default();
    r.check("F1: listed as virtual with what project_open returns", lj5["virtual"] == true && lj5["clip"] == open(&core, &p5.id).unwrap_or_default(), "");
    let p6 = create_legacy(&core, "Old empty");
    let files6 = n_files();
    let a6 = add(&core, &p6.id, vec![c.clone()]);
    r.check("an empty old project takes clips at once and becomes one of references", matches!(a6, Ok(Added::Now(_))) && get(&core, &p6.id).is_some_and(|p| p.virt && p.clips.len() == 1) && n_files() == files6, "");
    {
        // An entry exactly as 0.1.37 wrote it (no "virtual", clips with id, title, duration only).
        let _g = LOCK.lock().unwrap();
        let mut raw: Vec<Value> = serde_json::from_str(&fs::read_to_string(file()).unwrap_or_default()).unwrap_or_default();
        raw.push(json!({ "id": "old1", "name": "Old file", "created": 1, "modified": 1, "clips": [{ "id": a, "title": "Clip PA", "duration": 4.0 }], "exported": false }));
        let _ = fs::write(file(), serde_json::to_string(&raw).unwrap_or_default());
    }
    let o1 = get(&core, "old1");
    r.check("a 0.1.37 entry: read as an old project, its clip kept, its missing draft a clear refusal", o1.as_ref().is_some_and(|p| !p.virt && p.clips.len() == 1 && p.clips[0].size == 0 && p.clip.is_none() && p.clip_json.is_none()) && open(&core, "old1").is_err_and(|e| e == NO_DRAFT), &format!("{o1:?}"));
    r.check("a 0.1.37 entry: deleted as before", delete(&core, "old1") && !exists("old1"), "");

    // 8. The start-up sweep keeps project drafts and removes orphans.
    let p4 = create(&core, "Empty one");
    let drafts = root.join(".drafts");
    let w = |p: &Path| {
        let _ = fs::write(p, b"x");
        p.to_string_lossy().into_owned()
    };
    let orphan = w(&drafts.join("orphan.mp4"));
    let joining = w(&drafts.join(".y.mp4.joining"));
    let stray = w(&drafts.join("stray.mp4"));
    core.lib.lock().unwrap().add(ClipMeta { id: "stray".into(), path: stray.clone(), title: "stray".into(), kind: "joined".into(), draft: true, ..Default::default() });
    let _ = fs::write(states_dir().join("ghost.state.json"), "x");
    let keep = get(&core, &p1.id).and_then(|p| p.clip).unwrap_or_default();
    let n = sweep_at_start(&mut core.lib.lock().unwrap(), &root);
    r.check("sweep: a project's draft entry, file and thumbnail are kept", core.lib.lock().unwrap().get(&p1.id).is_some_and(|c| c.draft) && Path::new(&keep.path).is_file() && Path::new(&keep.thumb).is_file(), &keep.path);
    r.check("sweep: other files in .drafts and stray draft entries are removed", n == 3 && !Path::new(&orphan).exists() && !Path::new(&joining).exists() && !Path::new(&stray).exists() && core.lib.lock().unwrap().get("stray").is_none(), &format!("{n} removed"));
    r.check("sweep: a state file of no project is removed, a project's is kept", !states_dir().join("ghost.state.json").exists() && state(&p1.id).as_deref() == Some(text), "");
    r.check("sweep: an empty project stays", exists(&p4.id), "");
    core.jobs.shutdown();
    // As after a restart: a fresh library and core read it all back.
    let core2 = Core::new();
    let back = get(&core2, &p1.id).unwrap_or_default();
    r.check("after a restart: the project, its draft, joins and state are back", back.name == "My montage" && back.clip.as_ref().is_some_and(|c| c.joins == keep.joins && c.path == keep.path) && back.clips.len() == 3 && state(&p1.id).as_deref() == Some(text) && !core2.lib.lock().unwrap().visible().iter().any(|c| c.id == p1.id), &format!("{back:?}"));
    let ov = open(&core2, &v2.id);
    r.check("after a restart: a project of references opens with its sources, an old one with its draft", ov.as_ref().is_ok_and(|o| o["virtual"] == true && o["sources"].as_array().map(|s| s.len()) == Some(2) && o["sources"][0]["missing"] == false) && open(&core2, &p5.id).is_ok_and(|o| o["virtual"] == true && o["sources"].as_array().map(|s| s.len()) == Some(3) && o["sources"][0]["draft"] == true), &format!("{ov:?}"));
    *RECORD.lock().unwrap() = None;
    let _ = fs::remove_dir_all(&root);
}
