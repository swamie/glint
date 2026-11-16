//! What the editor prepares per source, made cheaper (0.1.41, CONTRACT_0141F F3). Owner: agent P.
//!
//! The expensive results (extracted tracks, waveforms, film strips) already have an on-disk cache: editor.rs keeps
//! them in `<tmp>\edit-<clip id>`, fresh while newer than the clip's file, swept by `remove_stale_dirs`. That one is
//! not doubled here. What this module adds, hooked into editor.rs by single anchored lines:
//!
//! - `titles`: the audio track titles of a file, remembered for the session by path + size + modified time. A warm
//!   `editor_audio_tracks` ran ffprobe every time just for them (about 60 ms per source, 19 sources at once).
//! - `slot` / `slot_light`: at most `SLOTS` film strips and `LIGHT_SLOTS` track extractions / waveforms that run
//!   ffmpeg at once (a project of 19 sources asks for all of them together: unbounded, all 18 film strips finished
//!   together after 46 s; bounded, the first ones are there after about 3 s, and sound rows never wait behind
//!   them). A cached answer never waits for a slot (the hooks sit after the cache checks).
//! - `cancel`: the editor closed or switched: whatever still waits for a slot gives up ("Cancelled"), so nothing
//!   runs for an editor that is gone. What already runs finishes (its result is cached for next time).
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::UNIX_EPOCH;

/// Film strips made at the same time (`slot`): each decodes frames across the whole clip, the heavy one (measured
/// 2.6 s of CPU for a 60 s 1440p60 clip), so few at once and the first ones arrive early.
pub const SLOTS: usize = 2;
/// Track extractions and waveforms at the same time (`slot_light`): stream copies and audio decodes, light, kept
/// apart from the film strips so a sound row never waits behind a picture strip.
pub const LIGHT_SLOTS: usize = 4;

static RUNNING: Mutex<usize> = Mutex::new(0);
static RUNNING_LIGHT: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();
static GEN: AtomicU64 = AtomicU64::new(0);

/// A held slot: given back when dropped.
pub struct Slot(&'static Mutex<usize>);

impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut n) = self.0.lock() {
            *n = n.saturating_sub(1);
        }
        FREED.notify_all();
    }
}

fn take(pool: &'static Mutex<usize>, max: usize) -> Result<Slot, String> {
    // `--speed-test` only: GLINT_EDCACHE_UNBOUNDED=1 measures the old way (every preparation at once).
    let max = if std::env::var_os("GLINT_EDCACHE_UNBOUNDED").is_some() { usize::MAX } else { max };
    let gen = GEN.load(Ordering::SeqCst);
    let mut n = pool.lock().map_err(|_| "Cancelled".to_string())?;
    while *n >= max {
        if GEN.load(Ordering::SeqCst) != gen {
            return Err("Cancelled".into());
        }
        n = FREED.wait_timeout(n, std::time::Duration::from_millis(200)).map_err(|_| "Cancelled".to_string())?.0;
    }
    if GEN.load(Ordering::SeqCst) != gen {
        return Err("Cancelled".into());
    }
    *n += 1;
    Ok(Slot(pool))
}

/// Wait for one of the `SLOTS` (film strips). Err("Cancelled") when `cancel` was called while it waited.
pub fn slot() -> Result<Slot, String> {
    take(&RUNNING, SLOTS)
}

/// Wait for one of the `LIGHT_SLOTS` (tracks, waveforms). Err("Cancelled") when `cancel` was called while it waited.
pub fn slot_light() -> Result<Slot, String> {
    take(&RUNNING_LIGHT, LIGHT_SLOTS)
}

/// Everything waiting for a slot now gives up (the editor closed or opened something else).
pub fn cancel() {
    GEN.fetch_add(1, Ordering::SeqCst);
    FREED.notify_all();
}

/// How many film-strip slots are taken (for the self-test).
pub fn running() -> usize {
    RUNNING.lock().map(|n| *n).unwrap_or(0)
}

type Titles = Vec<Option<String>>;
static TITLES: Mutex<Option<HashMap<String, (u64, u64, Titles)>>> = Mutex::new(None);
/// Files whose titles are remembered at most (the oldest go first when it is full: the map is simply cleared).
const TITLES_MAX: usize = 2000;

/// The file's (size, modified ms), or None when it cannot be read.
fn stamp(p: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
    Some((m.len(), t))
}

/// The audio track titles of `file`: remembered for the session by path + size + modified time, read with `read`
/// the first time (and after the file changed). An error is not remembered.
pub fn titles(file: &Path, read: impl FnOnce() -> Result<Titles, String>) -> Result<Titles, String> {
    let key = file.to_string_lossy().to_lowercase();
    let st = stamp(file);
    if let (Some((size, t)), Ok(g)) = (st, TITLES.lock()) {
        if let Some((s2, t2, v)) = g.as_ref().and_then(|m| m.get(&key)) {
            if *s2 == size && *t2 == t {
                return Ok(v.clone());
            }
        }
    }
    let v = read()?;
    if let (Some((size, t)), Ok(mut g)) = (st, TITLES.lock()) {
        let m = g.get_or_insert_with(HashMap::new);
        if m.len() >= TITLES_MAX {
            m.clear();
        }
        m.insert(key, (size, t, v.clone()));
    }
    Ok(v)
}

/// `--editor-test` / `--speed-test` checks of this module: (name, ok, detail).
pub fn checks(dir: &Path) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    let f = dir.join("edcache-titles.bin");
    let _ = std::fs::write(&f, b"one");
    let mut reads = 0;
    let a = titles(&f, || { reads += 1; Ok(vec![Some("Mix".into())]) });
    let b = titles(&f, || { reads += 1; Ok(vec![Some("Other".into())]) });
    out.push(("edcache: titles read once, then remembered (hit)".into(), reads == 1 && a == b, format!("{reads} reads")));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = std::fs::write(&f, b"changed: longer");
    let c = titles(&f, || { reads += 1; Ok(vec![Some("New".into())]) });
    out.push(("edcache: a changed file is read again (miss)".into(), reads == 2 && c == Ok(vec![Some("New".to_string())]), format!("{reads} reads")));
    let e = titles(&dir.join("edcache-none.bin"), || Err("nope".into()));
    out.push(("edcache: an error is passed on, not remembered".into(), e.is_err(), String::new()));
    let _ = std::fs::remove_file(&f);
    // Slots: SLOTS at once; one more waits; cancel makes the waiting one give up.
    let held: Vec<Slot> = (0..SLOTS).filter_map(|_| slot().ok()).collect();
    out.push(("edcache: SLOTS slots can be held at once".into(), held.len() == SLOTS && running() >= SLOTS, format!("{}", running())));
    let waiter = std::thread::spawn(slot);
    std::thread::sleep(std::time::Duration::from_millis(150));
    let still = !waiter.is_finished();
    cancel();
    let res = waiter.join().map(|r| r.err()).unwrap_or(None);
    out.push(("edcache: one more waits, and gives up when cancelled".into(), still && res.as_deref() == Some("Cancelled"), format!("{still} {res:?}")));
    drop(held);
    let again = slot();
    out.push(("edcache: a slot is free again once given back".into(), again.is_ok(), String::new()));
    out
}
