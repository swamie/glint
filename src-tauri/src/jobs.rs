//! Background renders: Blur renders ("blur") and Compress for Discord
//! ("compress", see compress.rs). A render is a job: it is queued, runs, and
//! ends as done, failed or cancelled. The UI lists jobs behind a button in the
//! top bar, so a render keeps going after its clip is closed.
//!
//! Contract with the UI (`ui/jobs.js`): the `list_jobs`, `cancel_job` and
//! `clear_jobs` commands in `app.rs`, a "jobs-changed" event carrying the whole
//! list (each job with its `kind`) whenever anything changes, and the older
//! "blur-progress" event
//! ({ id: <source clip id>, pct, state: "queued" | "running" | "done" | "error" | "cancelled", message })
//! that the library cards and the clip viewer already listen to. "blur-progress"
//! is sent for Blur jobs only, so a compress job never shows as a Blur render
//! on its clip's card; compress jobs are followed through "jobs-changed" and
//! their own "clip-compressed" event.
//!
//! How the queue works: every job is pushed to one list (oldest first) as
//! "queued", together with its `Runner`, the function that does the work.
//! Jobs run in lanes by kind: Blur jobs while fewer than the limit
//! (`settings.blur_parallel`, clamped 1..=4, read on every pump) Blur jobs are
//! running; compress jobs one at a time (`COMPRESS_PARALLEL`), next to the
//! Blur renders. So a compress never waits behind a long Blur queue, and two
//! x264 encodes never fight over the CPU (Blur works mostly on the GPU).
//! `pump` walks the list in order and starts each queued job whose lane has
//! room; each started job gets its own thread, and when its runner returns the
//! job ends and `pump` runs again. While jobs wait, a small waiter thread pumps
//! twice a second so a raised limit applies without waiting for a render to
//! end. Cancel sets the job's flag; the runners poll it and kill their
//! process (blur-cli's tree, or ffmpeg).
use crate::core::Core;
use crate::{blur, library, util};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Serialize, Clone, Debug, Default)]
pub struct Job {
    pub id: String,
    /// The clip being rendered.
    pub clip_id: String,
    /// That clip's title, for the list.
    pub title: String,
    /// "blur" (a Blur render, result kind "blur") or "compress" (Compress for
    /// Discord, result kind "discord"). Decides the job's lane in the queue.
    pub kind: String,
    /// "queued" | "running" | "done" | "error" | "cancelled"
    pub state: String,
    /// 0..100
    pub pct: f32,
    /// Why it failed, when state is "error".
    pub message: String,
    /// Id of the clip the render produced, when state is "done".
    pub result_id: String,
    /// Unix milliseconds when it was queued.
    pub queued: u64,
}

impl Job {
    fn ended(&self) -> bool {
        !matches!(self.state.as_str(), "queued" | "running")
    }
}

/// The work of one job. It gets a cancel flag to honour and a progress
/// callback (0..100), and returns the id of the clip it made or why it failed.
/// Returning any error while the flag is set ends the job as "cancelled".
pub type Runner = Box<dyn FnOnce(&AtomicBool, &mut dyn FnMut(f32)) -> Result<String, String> + Send>;

/// Ended jobs kept in the list.
const KEEP_ENDED: usize = 30;
/// Least time between two progress events of one job.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// How long `shutdown` waits for running renders to stop.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
/// Compress jobs running at once (their own lane, not counted against `blur_parallel`).
const COMPRESS_PARALLEL: usize = 1;

/// True for jobs of the compress lane; every other kind is a Blur render.
fn is_compress(kind: &str) -> bool {
    kind == "compress"
}

struct Hooks {
    limit: Box<dyn Fn() -> u32 + Send + Sync>,
    emit: Box<dyn Fn(&str, Value) + Send + Sync>,
}

struct Task {
    cancel: Arc<AtomicBool>,
    run: Option<Runner>,
    last_emit: Option<Instant>,
}

#[derive(Default)]
struct State {
    list: Vec<Job>,
    tasks: HashMap<String, Task>,
    closed: bool,
    waiter: bool,
}

struct Inner {
    state: Mutex<State>,
    hooks: OnceLock<Hooks>,
    /// Held while an event is sent, so "jobs-changed" lists arrive in order.
    emit_lock: Mutex<()>,
    /// Signalled whenever a running job ends.
    ended: Condvar,
}

/// The job list. One per app, held by `Core`.
pub struct Jobs {
    inner: Arc<Inner>,
}

impl Jobs {
    pub fn new() -> Self {
        Jobs { inner: Arc::new(Inner { state: Mutex::new(State::default()), hooks: OnceLock::new(), emit_lock: Mutex::new(()), ended: Condvar::new() }) }
    }

    /// Set how the queue reads its limit and sends events (first call wins).
    fn set_hooks(&self, limit: impl Fn() -> u32 + Send + Sync + 'static, emit: impl Fn(&str, Value) + Send + Sync + 'static) {
        let _ = self.inner.hooks.set(Hooks { limit: Box::new(limit), emit: Box::new(emit) });
    }

    /// Wire the queue to the app: the limit from the settings, events to the UI.
    /// Holds the core weakly, so the core and its job list do not keep each other alive.
    pub(crate) fn connect(&self, core: &Arc<Core>) {
        if self.inner.hooks.get().is_some() {
            return;
        }
        let (a, b) = (Arc::downgrade(core), Arc::downgrade(core));
        self.set_hooks(
            move || a.upgrade().map(|c| c.settings.lock().unwrap().blur_parallel).unwrap_or(2),
            move |event, payload| {
                if let Some(c) = b.upgrade() {
                    c.emit(event, payload);
                }
            },
        );
    }

    /// Every job, oldest first.
    pub fn list(&self) -> Vec<Job> {
        self.inner.state.lock().unwrap().list.clone()
    }

    /// Queue a job. Refused when the same clip already has a job of the same
    /// kind queued or running (a clip being blurred can still be compressed).
    pub fn enqueue(&self, clip_id: &str, title: &str, kind: &str, run: Runner) -> Result<Job, String> {
        let job = {
            let mut st = self.inner.state.lock().unwrap();
            if st.closed {
                return Err("Glint is closing".into());
            }
            if st.list.iter().any(|j| j.clip_id == clip_id && j.kind == kind && !j.ended()) {
                return Err(if is_compress(kind) { "That clip is already being compressed".into() } else { "That clip is already rendering".into() });
            }
            let job = Job { id: util::new_id(), clip_id: clip_id.into(), title: title.into(), kind: kind.into(), state: "queued".into(), queued: util::unix_ms(), ..Default::default() };
            st.tasks.insert(job.id.clone(), Task { cancel: Arc::new(AtomicBool::new(false)), run: Some(run), last_emit: None });
            st.list.push(job.clone());
            trim(&mut st.list);
            job
        };
        self.inner.blur_progress(&job);
        self.inner.publish();
        self.inner.pump();
        Ok(job)
    }

    /// Stop a queued or running job. Returns false if there is no such job or it has already ended.
    /// A queued job is cancelled at once; a running one is told to stop and
    /// becomes "cancelled" once its render has been killed (well under a second).
    pub fn cancel(&self, id: &str) -> bool {
        let job = {
            let mut st = self.inner.state.lock().unwrap();
            let Some(i) = st.list.iter().position(|j| j.id == id) else { return false };
            match st.list[i].state.as_str() {
                "queued" => {
                    st.list[i].state = "cancelled".into();
                    st.tasks.remove(id);
                    let job = st.list[i].clone();
                    trim(&mut st.list);
                    job
                }
                "running" => {
                    if let Some(task) = st.tasks.get(id) {
                        task.cancel.store(true, Ordering::SeqCst);
                    }
                    return true;
                }
                _ => return false,
            }
        };
        self.inner.blur_progress(&job);
        self.inner.publish();
        true
    }

    /// Drop the jobs that have ended (done, error, cancelled) from the list.
    pub fn clear_finished(&self) {
        self.inner.state.lock().unwrap().list.retain(|j| !j.ended());
        self.inner.publish();
    }

    /// Start waiting jobs if there is room (also done on its own; call it after
    /// the limit changes to apply it at once).
    #[allow(dead_code)]
    pub fn pump(&self) {
        self.inner.pump();
    }

    /// For app exit: start nothing more, cancel every waiting job, stop every
    /// running render (killing its blur-cli tree) and wait up to a few seconds
    /// for them to end. Afterwards no job is left queued or running.
    pub fn shutdown(&self) {
        let dropped: Vec<Job> = {
            let mut st = self.inner.state.lock().unwrap();
            st.closed = true;
            let mut dropped = Vec::new();
            let State { list, tasks, .. } = &mut *st;
            for j in list.iter_mut() {
                match j.state.as_str() {
                    "queued" => {
                        j.state = "cancelled".into();
                        tasks.remove(&j.id);
                        dropped.push(j.clone());
                    }
                    "running" => {
                        if let Some(task) = tasks.get(&j.id) {
                            task.cancel.store(true, Ordering::SeqCst);
                        }
                    }
                    _ => {}
                }
            }
            dropped
        };
        for j in &dropped {
            self.inner.blur_progress(j);
        }
        self.inner.publish();
        let st = self.inner.state.lock().unwrap();
        let (mut st, _) = self.inner.ended.wait_timeout_while(st, SHUTDOWN_WAIT, |st| st.list.iter().any(|j| j.state == "running")).unwrap();
        // A render that would not stop in time: still report it as cancelled.
        let stuck: Vec<Job> = st
            .list
            .iter_mut()
            .filter(|j| j.state == "running")
            .map(|j| {
                j.state = "cancelled".into();
                j.clone()
            })
            .collect();
        drop(st);
        if !stuck.is_empty() {
            for j in &stuck {
                self.inner.blur_progress(j);
            }
            self.inner.publish();
        }
    }
}

/// Keep at most `KEEP_ENDED` ended jobs (the newest); unfinished ones always stay.
fn trim(list: &mut Vec<Job>) {
    let mut extra = list.iter().filter(|j| j.ended()).count().saturating_sub(KEEP_ENDED);
    list.retain(|j| {
        if extra > 0 && j.ended() {
            extra -= 1;
            false
        } else {
            true
        }
    });
}

impl Inner {
    fn limit(&self) -> usize {
        self.hooks.get().map(|h| (h.limit)()).unwrap_or(2).clamp(1, 4) as usize
    }

    fn emit(&self, event: &str, payload: Value) {
        if let Some(h) = self.hooks.get() {
            (h.emit)(event, payload);
        }
    }

    /// Send the whole list as "jobs-changed".
    fn publish(&self) {
        let _order = self.emit_lock.lock().unwrap();
        let list = self.state.lock().unwrap().list.clone();
        self.emit("jobs-changed", serde_json::to_value(&list).unwrap_or(Value::Null));
    }

    /// The older per-clip event the library cards and the viewer use (Blur jobs only).
    fn blur_progress(&self, j: &Job) {
        if is_compress(&j.kind) {
            return;
        }
        let message = if j.state == "done" { &j.result_id } else { &j.message };
        let _order = self.emit_lock.lock().unwrap();
        self.emit("blur-progress", json!({ "id": j.clip_id, "pct": j.pct, "state": j.state, "message": message }));
    }

    /// Start queued jobs, oldest first, while their lane has room.
    fn pump(self: &Arc<Self>) {
        let mut started: Vec<(Job, Runner, Arc<AtomicBool>)> = Vec::new();
        let mut failed: Vec<Job> = Vec::new();
        let need_waiter = {
            let mut st = self.state.lock().unwrap();
            if st.closed {
                return;
            }
            let limit = self.limit();
            // Running jobs per lane: [blur, compress].
            let mut running = [0usize; 2];
            for j in st.list.iter().filter(|j| j.state == "running") {
                running[is_compress(&j.kind) as usize] += 1;
            }
            let caps = [limit, COMPRESS_PARALLEL];
            for i in 0..st.list.len() {
                if st.list[i].state != "queued" {
                    continue;
                }
                let lane = is_compress(&st.list[i].kind) as usize;
                if running[lane] >= caps[lane] {
                    continue;
                }
                let id = st.list[i].id.clone();
                match st.tasks.get_mut(&id).and_then(|t| t.run.take().map(|r| (r, t.cancel.clone()))) {
                    Some((run, cancel)) => {
                        st.list[i].state = "running".into();
                        st.list[i].pct = 0.0;
                        started.push((st.list[i].clone(), run, cancel));
                        running[lane] += 1;
                    }
                    None => {
                        st.list[i].state = "error".into();
                        st.list[i].message = "The job was lost".into();
                        st.tasks.remove(&id);
                        failed.push(st.list[i].clone());
                    }
                }
            }
            let waiting = st.list.iter().any(|j| j.state == "queued");
            if waiting && !st.waiter {
                st.waiter = true;
                true
            } else {
                false
            }
        };
        for j in &failed {
            self.blur_progress(j);
        }
        let changed = !failed.is_empty() || !started.is_empty();
        for (job, run, cancel) in started {
            self.blur_progress(&job);
            let me = self.clone();
            thread::spawn(move || me.work(job.id, run, cancel));
        }
        if changed {
            self.publish();
        }
        if need_waiter {
            let me = self.clone();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_millis(500));
                {
                    let mut st = me.state.lock().unwrap();
                    if st.closed || !st.list.iter().any(|j| j.state == "queued") {
                        st.waiter = false;
                        return;
                    }
                }
                me.pump();
            });
        }
    }

    /// One running job, on its own thread.
    fn work(self: Arc<Self>, id: String, run: Runner, cancel: Arc<AtomicBool>) {
        let me = self.clone();
        let job_id = id.clone();
        let mut progress = move |pct: f32| me.progress(&job_id, pct);
        let flag = cancel.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || run(&flag, &mut progress))).unwrap_or_else(|_| Err("The render crashed".into()));
        let cancelled = cancel.load(Ordering::SeqCst);
        let job = {
            let mut st = self.state.lock().unwrap();
            st.tasks.remove(&id);
            let job = st.list.iter_mut().find(|j| j.id == id).filter(|j| j.state == "running").map(|j| {
                match result {
                    Ok(rid) => {
                        j.state = "done".into();
                        j.pct = 100.0;
                        j.result_id = rid;
                    }
                    Err(_) if cancelled => j.state = "cancelled".into(),
                    Err(e) => {
                        j.state = "error".into();
                        j.message = e;
                    }
                }
                j.clone()
            });
            trim(&mut st.list);
            self.ended.notify_all();
            job
        };
        if let Some(job) = job {
            self.blur_progress(&job);
            self.publish();
        }
        self.pump();
    }

    /// Progress of a running job: never backwards, sent at most every `PROGRESS_EVERY`.
    fn progress(&self, id: &str, pct: f32) {
        let pct = if pct.is_finite() { pct.clamp(0.0, 100.0) } else { return };
        let job = {
            let mut st = self.state.lock().unwrap();
            let State { list, tasks, .. } = &mut *st;
            let Some(j) = list.iter_mut().find(|j| j.id == id && j.state == "running") else { return };
            if pct <= j.pct {
                return;
            }
            j.pct = pct;
            let Some(task) = tasks.get_mut(id) else { return };
            if task.last_emit.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) {
                return;
            }
            task.last_emit = Some(Instant::now());
            j.clone()
        };
        self.blur_progress(&job);
        self.publish();
    }
}

/// Output files picked by running renders that blur has not created yet.
fn reserved() -> &'static Mutex<HashSet<PathBuf>> {
    static R: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashSet::new()))
}

/// First free `base.ext` / `base (n).ext` in `dir`, not on disk and not picked by another render.
fn reserve_output(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut r = reserved().lock().unwrap();
    let mut n = 1;
    loop {
        let p = if n == 1 { dir.join(format!("{base}.{ext}")) } else { dir.join(format!("{base} ({n}).{ext}")) };
        if !p.exists() && !r.contains(&p) {
            r.insert(p.clone());
            return p;
        }
        n += 1;
    }
}

/// Queue a Blur render of clip `clip_id`. At most `settings.blur_parallel`
/// renders run at once; the rest wait their turn.
pub fn enqueue_blur(core: &Arc<Core>, clip_id: String) -> Result<Job, String> {
    let meta = core.lib.lock().unwrap().get(&clip_id).ok_or("Clip not found")?;
    let s = core.settings();
    let blur_cli = blur::find(&s).ok_or("blur-cli.exe was not found. Install Blur or set its path under Blur settings.")?;
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg").ok_or("ffmpeg.exe was not found")?;
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let m = meta.clone();
    let run: Runner = Box::new(move |cancel, progress| {
        let core = weak.upgrade().ok_or("Glint is closing")?;
        let src = PathBuf::from(&m.path);
        if !src.is_file() {
            return Err("The clip's video file is missing".into());
        }
        let dir = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from(&s.clips_dir));
        let title = format!("{} (blur)", m.title);
        let out = reserve_output(&dir, &util::sanitize_filename(&title), &blur::container(&s.blur));
        let result = blur::render_cancellable(&blur_cli, &src, &out, &s.blur, cancel, |pct| progress(pct));
        reserved().lock().unwrap().remove(&out);
        result?;
        let mut new = library::register(&ffmpeg, &out, &title, &m.game, "blur");
        new.folder = m.folder.clone();
        core.lib.lock().unwrap().add(new.clone());
        core.emit("clips-changed", new.id.clone());
        Ok(new.id)
    });
    core.jobs.enqueue(&clip_id, &meta.title, "blur", run)
}

// ---------------------------------------------------------------------------
// Self-test

struct Report {
    path: PathBuf,
    failed: usize,
}

impl Report {
    fn line(&mut self, s: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{s}");
        }
    }
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if !ok {
            self.failed += 1;
        }
        let d = if ok || detail.is_empty() { String::new() } else { format!(" ({detail})") };
        self.line(&format!("{} {name}{d}", if ok { "PASS" } else { "FAIL" }));
    }
}

/// What the fake runners did.
#[derive(Default)]
struct Tracker {
    cur: usize,
    max: usize,
    order: Vec<String>,
    /// Other runners active when each one started.
    at_start: HashMap<String, usize>,
}

type Track = Arc<Mutex<Tracker>>;
type Events = Arc<Mutex<Vec<(String, Value)>>>;

struct Rig {
    jobs: Jobs,
    limit: Arc<std::sync::atomic::AtomicU32>,
    events: Events,
    track: Track,
}

fn rig(limit: u32) -> Rig {
    let jobs = Jobs::new();
    let lim = Arc::new(std::sync::atomic::AtomicU32::new(limit));
    let events: Events = Arc::default();
    let (l, e) = (lim.clone(), events.clone());
    jobs.set_hooks(move || l.load(Ordering::SeqCst), move |name, payload| e.lock().unwrap().push((name.to_string(), payload)));
    Rig { jobs, limit: lim, events, track: Arc::default() }
}

/// A runner that never touches blur-cli: `steps` sleeps of `step_ms`, reporting
/// progress (in the order `pcts` gives, or evenly), honouring the cancel flag.
fn fake(track: &Track, name: &str, steps: u32, step_ms: u64, fail: Option<&str>, pcts: Option<Vec<f32>>) -> Runner {
    let (track, name, fail) = (track.clone(), name.to_string(), fail.map(String::from));
    Box::new(move |cancel, progress| {
        {
            let mut t = track.lock().unwrap();
            let c = t.cur;
            t.at_start.insert(name.clone(), c);
            t.cur += 1;
            t.max = t.max.max(t.cur);
            t.order.push(name.clone());
        }
        let end = |t: &Track| t.lock().unwrap().cur -= 1;
        for i in 1..=steps {
            if cancel.load(Ordering::SeqCst) {
                end(&track);
                return Err(blur::CANCELLED.into());
            }
            thread::sleep(Duration::from_millis(step_ms));
            let p = pcts.as_ref().and_then(|v| v.get(i as usize - 1).copied()).unwrap_or(i as f32 * 100.0 / steps as f32);
            progress(p);
        }
        end(&track);
        match fail {
            Some(m) => Err(m),
            None => Ok(format!("res-{name}")),
        }
    })
}

fn wait_until(secs: f64, mut cond: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    cond()
}

fn state_of(jobs: &Jobs, clip: &str) -> String {
    jobs.list().iter().rev().find(|j| j.clip_id == clip).map(|j| j.state.clone()).unwrap_or_default()
}

fn running(jobs: &Jobs) -> usize {
    jobs.list().iter().filter(|j| j.state == "running").count()
}

fn all_ended(jobs: &Jobs) -> bool {
    jobs.list().iter().all(Job::ended)
}

/// The "blur-progress" payloads sent for one clip.
fn progress_events(events: &Events, clip: &str) -> Vec<Value> {
    events.lock().unwrap().iter().filter(|(n, p)| n == "blur-progress" && p["id"] == clip).map(|(_, p)| p.clone()).collect()
}

/// `glint.exe --jobs-test <report.txt>`: headless checks of the queue logic.
/// Uses fake runners only: blur-cli is never started.
pub fn self_test(report_path: &str) {
    let mut r = Report { path: PathBuf::from(report_path), failed: 0 };
    let _ = std::fs::remove_file(&r.path);
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        r.check("refused", false, "set GLINT_DATA_DIR so the real library and settings are never touched");
        return;
    }
    run_self_test(&mut r);
    let failed = r.failed;
    r.line(&format!("DONE failed={failed}"));
}

fn run_self_test(r: &mut Report) {
    // 1. Limit and FIFO order.
    {
        let g = rig(2);
        for i in 0..6 {
            let name = format!("c{i}");
            let _ = g.jobs.enqueue(&name, &name, "blur", fake(&g.track, &name, 5, 30, None, None));
        }
        let ok = wait_until(15.0, || all_ended(&g.jobs));
        r.check("limit 2: six jobs all end", ok, "");
        let t = g.track.lock().unwrap();
        r.check("limit 2: never more than 2 renders at once", t.max <= 2, &format!("max {}", t.max));
        r.check("limit 2: 2 renders do run side by side", t.max == 2, &format!("max {}", t.max));
        // FIFO, judged on lock-consistent snapshots (runner threads may be scheduled in any order):
        // in no list does a waiting job sit before one that has started.
        let fifo = g.events.lock().unwrap().iter().filter(|(n, _)| n == "jobs-changed").all(|(_, p)| {
            let states: Vec<&str> = p.as_array().map(|a| a.iter().map(|j| j["state"].as_str().unwrap_or("")).collect()).unwrap_or_default();
            states.iter().position(|s| *s == "queued").map(|q| states[q..].iter().all(|s| *s == "queued")).unwrap_or(true)
        });
        r.check("FIFO: no job starts before an older waiting one", fifo, "");
        let mut seen = t.order.clone();
        seen.sort();
        r.check("FIFO: every job started exactly once", seen == (0..6).map(|i| format!("c{i}")).collect::<Vec<_>>(), &format!("{:?}", t.order));
        let lists_ok = g.events.lock().unwrap().iter().filter(|(n, _)| n == "jobs-changed").all(|(_, p)| p.as_array().map(|a| a.iter().filter(|j| j["state"] == "running").count() <= 2).unwrap_or(false));
        r.check("limit 2: no jobs-changed list shows more than 2 running", lists_ok, "");
        let list = g.jobs.list();
        r.check("all six done with their result id", list.iter().all(|j| j.state == "done" && j.result_id == format!("res-{}", j.clip_id)), "");
        let queued_seen = progress_events(&g.events, "c5").iter().any(|p| p["state"] == "queued");
        r.check("blur-progress says \"queued\" for a waiting job", queued_seen, "");
    }

    // 2. Raising and lowering the limit.
    {
        let g = rig(1);
        for i in 0..4 {
            let name = format!("r{i}");
            let _ = g.jobs.enqueue(&name, &name, "blur", fake(&g.track, &name, 25, 40, None, None));
        }
        thread::sleep(Duration::from_millis(200));
        r.check("limit 1: one running, three queued", running(&g.jobs) == 1, &format!("{} running", running(&g.jobs)));
        g.limit.store(3, Ordering::SeqCst);
        let ok = wait_until(2.0, || running(&g.jobs) == 3);
        r.check("raising the limit to 3 starts waiting jobs without a render ending", ok, &format!("{} running", running(&g.jobs)));
        g.limit.store(1, Ordering::SeqCst);
        let ok = wait_until(20.0, || all_ended(&g.jobs));
        r.check("lowered limit: all end", ok, "");
        let t = g.track.lock().unwrap();
        let at = t.at_start.get("r3").copied();
        r.check("lowering the limit to 1: the last job waits until every render has ended", at == Some(0), &format!("{at:?} running at its start"));
        r.check("limit change: first and last in order", t.order.first().map(String::as_str) == Some("r0") && t.order.last().map(String::as_str) == Some("r3"), &format!("{:?}", t.order));
    }

    // 3. Cancel a queued job, then a running one.
    {
        let g = rig(1);
        let a = g.jobs.enqueue("a", "a", "blur", fake(&g.track, "a", 200, 20, None, None)).unwrap();
        let b = g.jobs.enqueue("b", "b", "blur", fake(&g.track, "b", 2, 20, None, None)).unwrap();
        let _ = g.jobs.enqueue("c", "c", "blur", fake(&g.track, "c", 5, 20, None, None)).unwrap();
        wait_until(2.0, || state_of(&g.jobs, "a") == "running");
        r.check("cancel of a queued job returns true", g.jobs.cancel(&b.id), "");
        r.check("a cancelled queued job is \"cancelled\" at once", state_of(&g.jobs, "b") == "cancelled", &state_of(&g.jobs, "b"));
        r.check("cancel of an ended job returns false", !g.jobs.cancel(&b.id), "");
        r.check("cancel of an unknown job returns false", !g.jobs.cancel("no-such-job"), "");
        r.check("cancelled queued job sent blur-progress \"cancelled\"", progress_events(&g.events, "b").iter().any(|p| p["state"] == "cancelled"), "");
        let t0 = Instant::now();
        r.check("cancel of a running job returns true", g.jobs.cancel(&a.id), "");
        let ok = wait_until(2.0, || state_of(&g.jobs, "a") == "cancelled");
        r.check("a cancelled running job becomes \"cancelled\"", ok, &format!("{} after {:?}", state_of(&g.jobs, "a"), t0.elapsed()));
        let ok = wait_until(2.0, || state_of(&g.jobs, "c") != "queued");
        r.check("cancelling a running job frees its slot for the next", ok, &state_of(&g.jobs, "c"));
        wait_until(5.0, || all_ended(&g.jobs));
        r.check("the next job then finishes", state_of(&g.jobs, "c") == "done", &state_of(&g.jobs, "c"));
        let t = g.track.lock().unwrap();
        r.check("a cancelled queued job never starts", !t.order.contains(&"b".to_string()), &format!("{:?}", t.order));
        r.check("cancelled running job sent blur-progress \"cancelled\"", progress_events(&g.events, "a").last().map(|p| p["state"] == "cancelled").unwrap_or(false), "");
    }

    // 4. A failing job and a crashing one; the queue carries on.
    {
        let g = rig(1);
        let _ = g.jobs.enqueue("f", "f", "blur", fake(&g.track, "f", 3, 20, Some("Boom happened"), None));
        let _ = g.jobs.enqueue("p", "p", "blur", Box::new(|_c: &AtomicBool, _p: &mut dyn FnMut(f32)| -> Result<String, String> { panic!("test panic") }));
        let _ = g.jobs.enqueue("g", "g", "blur", fake(&g.track, "g", 3, 20, None, None));
        wait_until(5.0, || all_ended(&g.jobs));
        let f = g.jobs.list().into_iter().find(|j| j.clip_id == "f").unwrap_or_default();
        r.check("a failing job is \"error\" with its message", f.state == "error" && f.message == "Boom happened", &format!("{} / {}", f.state, f.message));
        let last = progress_events(&g.events, "f").last().cloned().unwrap_or(Value::Null);
        r.check("blur-progress \"error\" carries the message", last["state"] == "error" && last["message"] == "Boom happened", &last.to_string());
        r.check("a crashing runner is \"error\", not stuck running", state_of(&g.jobs, "p") == "error", &state_of(&g.jobs, "p"));
        r.check("the queue carries on after a failure", state_of(&g.jobs, "g") == "done", &state_of(&g.jobs, "g"));
    }

    // 5. Duplicates.
    {
        let g = rig(2);
        let first = g.jobs.enqueue("x", "x", "blur", fake(&g.track, "x", 10, 20, None, None));
        let second = g.jobs.enqueue("x", "x", "blur", fake(&g.track, "x", 1, 1, None, None));
        r.check("first render of a clip is accepted", first.is_ok(), "");
        r.check("second render of a running clip is refused", second.as_ref().err().map(String::as_str) == Some("That clip is already rendering"), &format!("{second:?}"));
        let _ = g.jobs.enqueue("y1", "y1", "blur", fake(&g.track, "y1", 10, 20, None, None));
        let _ = g.jobs.enqueue("y", "y", "blur", fake(&g.track, "y", 1, 1, None, None));
        let dup = g.jobs.enqueue("y", "y", "blur", fake(&g.track, "y", 1, 1, None, None));
        r.check("second render of a queued clip is refused", dup.is_err(), "");
        wait_until(5.0, || all_ended(&g.jobs));
        let again = g.jobs.enqueue("x", "x", "blur", fake(&g.track, "x", 1, 1, None, None));
        r.check("a clip can be rendered again once its render has ended", again.is_ok(), &format!("{again:?}"));
        wait_until(5.0, || all_ended(&g.jobs));
    }

    // 6. Progress.
    {
        let g = rig(1);
        let _ = g.jobs.enqueue("m", "m", "blur", fake(&g.track, "m", 6, 120, None, Some(vec![10.0, 50.0, 30.0, 70.0, 20.0, 90.0])));
        let _ = g.jobs.enqueue("s", "s", "blur", fake(&g.track, "s", 100, 10, None, None));
        wait_until(10.0, || all_ended(&g.jobs));
        let ev = progress_events(&g.events, "m");
        let pcts: Vec<f64> = ev.iter().map(|p| p["pct"].as_f64().unwrap_or(-1.0)).collect();
        r.check("progress never goes backwards", pcts.windows(2).all(|w| w[1] >= w[0]), &format!("{pcts:?}"));
        let last = ev.last().cloned().unwrap_or(Value::Null);
        r.check("a finished job ends at 100 with state \"done\"", last["state"] == "done" && last["pct"].as_f64() == Some(100.0) && last["message"] == "res-m", &last.to_string());
        let m = g.jobs.list().into_iter().find(|j| j.clip_id == "m").unwrap_or_default();
        r.check("the job itself ends at 100", m.pct == 100.0, &m.pct.to_string());
        let n = progress_events(&g.events, "s").iter().filter(|p| p["state"] == "running").count();
        r.check("progress events are throttled (about 4 a second)", n <= 9, &format!("{n} running events over about 1 s"));
        let changed = g.events.lock().unwrap().iter().filter(|(n, _)| n == "jobs-changed").count();
        r.check("jobs-changed is sent", changed > 0, "");
    }

    // 7. clear_finished and the cap on ended jobs.
    {
        let g = rig(1);
        let _ = g.jobs.enqueue("d1", "d1", "blur", fake(&g.track, "d1", 1, 5, None, None));
        wait_until(3.0, || state_of(&g.jobs, "d1") == "done");
        let _ = g.jobs.enqueue("d2", "d2", "blur", fake(&g.track, "d2", 300, 20, None, None));
        let _ = g.jobs.enqueue("d3", "d3", "blur", fake(&g.track, "d3", 1, 5, None, None));
        wait_until(2.0, || state_of(&g.jobs, "d2") == "running");
        g.jobs.clear_finished();
        let ids: Vec<String> = g.jobs.list().iter().map(|j| j.clip_id.clone()).collect();
        r.check("clear_finished drops ended jobs and keeps unfinished ones", ids == ["d2", "d3"], &format!("{ids:?}"));

        // 8. shutdown.
        let t0 = Instant::now();
        g.jobs.shutdown();
        let el = t0.elapsed();
        let list = g.jobs.list();
        r.check("shutdown leaves nothing queued or running", list.iter().all(|j| j.state == "cancelled"), &format!("{:?}", list.iter().map(|j| &j.state).collect::<Vec<_>>()));
        r.check("shutdown stops the running render (runner returned)", g.track.lock().unwrap().cur == 0, "");
        r.check("shutdown returns promptly", el < Duration::from_secs(2), &format!("{el:?}"));
        r.check("a queued job never starts after shutdown", !g.track.lock().unwrap().order.contains(&"d3".to_string()), "");
        r.check("enqueue after shutdown is refused", g.jobs.enqueue("z", "z", "blur", fake(&g.track, "z", 1, 1, None, None)).is_err(), "");
        thread::sleep(Duration::from_millis(100));
        r.check("nothing starts after shutdown", running(&g.jobs) == 0, "");
    }
    {
        let g = rig(4);
        for i in 0..35 {
            let name = format!("k{i:02}");
            let _ = g.jobs.enqueue(&name, &name, "blur", fake(&g.track, &name, 1, 1, None, None));
        }
        wait_until(10.0, || all_ended(&g.jobs) && g.jobs.list().len() <= 30);
        let list = g.jobs.list();
        r.check("ended jobs are capped at 30", list.len() == 30, &list.len().to_string());
        r.check("the cap keeps the most recent", list.first().map(|j| j.clip_id.as_str()) == Some("k05") && list.last().map(|j| j.clip_id.as_str()) == Some("k34"), "");
    }

    // 9. Lanes: compress jobs do not wait behind the Blur limit, run one at a time,
    //    may share a clip with a Blur render and send no "blur-progress".
    {
        let g = rig(1);
        let _ = g.jobs.enqueue("L1", "L1", "blur", fake(&g.track, "L1", 60, 20, None, None));
        let _ = g.jobs.enqueue("L2", "L2", "blur", fake(&g.track, "L2", 2, 20, None, None));
        let c1 = g.jobs.enqueue("L1", "L1", "compress", fake(&g.track, "C1", 30, 20, None, None));
        r.check("lanes: a clip being blurred can still be compressed", c1.is_ok(), &format!("{c1:?}"));
        let dup = g.jobs.enqueue("L1", "L1", "compress", fake(&g.track, "C1b", 1, 1, None, None));
        r.check("lanes: a second compress of the same clip is refused", dup.as_ref().err().map(String::as_str) == Some("That clip is already being compressed"), &format!("{dup:?}"));
        let _ = g.jobs.enqueue("C2", "C2", "compress", fake(&g.track, "C2", 2, 20, None, None));
        let kind_state = |kind: &str, clip: &str| g.jobs.list().iter().rev().find(|j| j.clip_id == clip && j.kind == kind).map(|j| j.state.clone()).unwrap_or_default();
        let ok = wait_until(2.0, || kind_state("compress", "L1") == "running");
        r.check("lanes: a compress starts while the Blur slot is full and Blur jobs wait", ok && kind_state("blur", "L2") == "queued", &format!("compress {} / blur L2 {}", kind_state("compress", "L1"), kind_state("blur", "L2")));
        r.check("lanes: only one compress runs at a time", kind_state("compress", "C2") == "queued", &kind_state("compress", "C2"));
        let ok = wait_until(10.0, || all_ended(&g.jobs));
        r.check("lanes: every job ends", ok, "");
        let at = g.track.lock().unwrap().at_start.get("C2").copied();
        r.check("lanes: the second compress started after the first ended (only the Blur render beside it)", at.is_some_and(|n| n <= 1), &format!("{at:?} running at its start"));
        r.check("lanes: no blur-progress for a compress job", !progress_events(&g.events, "C2").iter().any(|_| true), "");
        let done = g.jobs.list().iter().filter(|j| j.state == "done").count();
        r.check("lanes: all four done", done == 4, &done.to_string());
    }
}
