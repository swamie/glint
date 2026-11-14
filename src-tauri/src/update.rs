//! Updates from GitHub releases.
//!
//! Glint asks GitHub for the latest release of `REPO`, at most once a day on
//! its own and whenever the user presses the update button. If its version is
//! newer than this build, the user can have it downloaded and put in place of
//! the running program, after which Glint restarts.
//!
//! Network use: one HTTPS request to api.github.com per check and, on install,
//! downloads from github.com. Nothing about the user or the PC is sent. It
//! uses curl.exe and tar.exe, which ship with Windows 10 and 11.
//!
//! A release must carry the asset `ASSET` (a zip with Glint.exe and
//! WebView2Loader.dll at its top level) and may carry `ASSET.sha256` (the
//! zip's SHA-256 in hex), which is then checked.
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const REPO: &str = "swamie/glint";
pub const ASSET: &str = "Glint-windows-x64.zip";
/// Seconds between automatic checks.
const CHECK_EVERY: u64 = 20 * 3600;

/// What is known about updates. Saved in `update.json` so a check done today is not repeated on the next start.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct UpdateInfo {
    /// Version of this build, e.g. "0.1.0".
    pub current: String,
    /// Version of the latest release ("" until a check has succeeded).
    pub latest: String,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// The release's notes, as written on GitHub.
    pub notes: String,
    /// Where to download `ASSET`, and its checksum file ("" if the release has none).
    pub url: String,
    pub sha_url: String,
    /// The release's page on GitHub.
    pub page: String,
    /// Unix seconds of the last successful check (0 = never).
    pub checked: u64,
    /// Why the last check failed ("" if it worked).
    pub error: String,
}

fn file() -> PathBuf {
    util::data_dir().join("update.json")
}

/// The last known state, with `current` and `available` recomputed for this build.
pub fn load() -> UpdateInfo {
    let mut info: UpdateInfo = fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    info.current = env!("CARGO_PKG_VERSION").to_string();
    info.available = newer(&info.latest, &info.current);
    info
}

fn save(info: &UpdateInfo) {
    if let Ok(t) = serde_json::to_string_pretty(info) {
        let _ = fs::write(file(), t);
    }
}

/// True if an automatic check is due (none in the last 20 hours).
pub fn due(info: &UpdateInfo) -> bool {
    util::unix_ms() / 1000 >= info.checked + CHECK_EVERY
}

/// "v1.2.3" or "1.2.3-beta" -> [1, 2, 3]. Missing or odd parts count as 0.
fn parts(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0))
        .collect()
}

/// True if version `a` is newer than version `b`.
pub fn newer(a: &str, b: &str) -> bool {
    if a.trim().is_empty() {
        return false;
    }
    let (mut x, mut y) = (parts(a), parts(b));
    x.resize(3, 0);
    y.resize(3, 0);
    x > y
}

fn curl() -> std::process::Command {
    let mut c = util::hidden_cmd(Path::new("curl.exe"));
    // A generic client name: GitHub requires a User-Agent, and nothing here should identify the user.
    c.args(["-sS", "-L", "--fail", "--retry", "2", "-H", "User-Agent: Glint-updater"]);
    c.stdin(Stdio::null());
    c
}

/// Ask GitHub for the latest release. Saves and returns the new state; on failure the old state is kept with `error` set.
pub fn check() -> UpdateInfo {
    let mut info = load();
    let out = curl()
        .args(["--max-time", "20", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .stderr(Stdio::piped())
        .output();
    let body = match out {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => {
            let why = util::last_lines(&o.stderr, 1);
            // 404 = the repository has no release yet: that is "up to date", not a failure.
            if why.contains("404") {
                info.error.clear();
                info.checked = util::unix_ms() / 1000;
                save(&info);
                return info;
            }
            info.error = format!("Could not reach GitHub: {why}");
            return info;
        }
        Err(e) => {
            info.error = format!("Could not run curl.exe: {e}");
            return info;
        }
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) else {
        info.error = "GitHub sent an answer Glint could not read".into();
        return info;
    };
    let asset_url = |name: &str| {
        json["assets"]
            .as_array()
            .and_then(|list| list.iter().find(|a| a["name"].as_str() == Some(name)))
            .and_then(|a| a["browser_download_url"].as_str())
            .unwrap_or("")
            .to_string()
    };
    info.latest = json["tag_name"].as_str().unwrap_or("").trim_start_matches(['v', 'V']).to_string();
    info.notes = json["body"].as_str().unwrap_or("").chars().take(4000).collect();
    info.page = json["html_url"].as_str().unwrap_or("").to_string();
    info.url = asset_url(ASSET);
    info.sha_url = asset_url(&format!("{ASSET}.sha256"));
    // A release without the zip (still uploading, or a source-only tag) cannot be installed.
    info.available = newer(&info.latest, &info.current) && !info.url.is_empty();
    info.checked = util::unix_ms() / 1000;
    info.error.clear();
    save(&info);
    info
}

fn sha256(file: &Path) -> Option<String> {
    let out = util::hidden_cmd(Path::new("certutil.exe")).arg("-hashfile").arg(file).arg("SHA256").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().replace(' ', "").to_lowercase())
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Download the release in `info` and put its files in place of the ones in
/// `dir` (the folder of the running Glint.exe). Files in use cannot be
/// overwritten on Windows but can be renamed, so each old file becomes
/// `<name>.old` (removed at the next start) and the new one takes its name.
/// `progress` gets 0..100 while downloading. Does not restart anything.
pub fn install_into(info: &UpdateInfo, dir: &Path, progress: &mut dyn FnMut(f32)) -> Result<(), String> {
    if info.url.is_empty() {
        return Err("This release has no download for Windows".into());
    }
    // Only ever fetch from GitHub: the address came from GitHub's own answer, but check anyway.
    if !info.url.starts_with("https://github.com/") {
        return Err("The download address is not on github.com".into());
    }
    let work = util::tmp_dir().join(format!("update-{}", util::new_id()));
    fs::create_dir_all(&work).map_err(|e| format!("Cannot create a temporary folder: {e}"))?;
    let result = (|| {
        let zip = work.join(ASSET);
        progress(1.0);
        let mut child = curl().args(["--max-time", "600", "-o"]).arg(&zip).arg(&info.url).stderr(Stdio::piped()).spawn().map_err(|e| format!("Could not run curl.exe: {e}"))?;
        // curl reports nothing useful when silent, so watch the file grow. The size is not known
        // here, so the bar creeps towards 90 % and jumps when the download ends.
        let mut shown = 1.0f32;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    shown += (90.0 - shown) * 0.08;
                    progress(shown);
                }
                Err(e) => return Err(format!("Download failed: {e}")),
            }
        }
        let out = child.wait_with_output().map_err(|e| format!("Download failed: {e}"))?;
        if !out.status.success() || !zip.is_file() {
            return Err(format!("Download failed: {}", util::last_lines(&out.stderr, 1)));
        }
        progress(92.0);

        if !info.sha_url.is_empty() && info.sha_url.starts_with("https://github.com/") {
            let want = curl().args(["--max-time", "30"]).arg(&info.sha_url).stderr(Stdio::null()).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
            let want = want.split_whitespace().next().unwrap_or("").to_lowercase();
            let got = sha256(&zip).unwrap_or_default();
            if want.len() == 64 && !got.is_empty() && want != got {
                return Err("The download does not match its checksum, so it was not installed".into());
            }
        }

        let unpacked = work.join("new");
        fs::create_dir_all(&unpacked).map_err(|e| format!("Cannot unpack the update: {e}"))?;
        let ok = util::hidden_cmd(Path::new("tar.exe")).arg("-xf").arg(&zip).arg("-C").arg(&unpacked).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        let new_exe = unpacked.join("Glint.exe");
        if !ok || fs::metadata(&new_exe).map(|m| m.len()).unwrap_or(0) < 1_000_000 {
            return Err("The update could not be unpacked".into());
        }
        progress(96.0);

        // Swap every file of the release in. On any failure, put the old files back.
        let files: Vec<PathBuf> = fs::read_dir(&unpacked).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        let mut swapped: Vec<(PathBuf, PathBuf)> = Vec::new(); // (target, its .old)
        for src in &files {
            let Some(name) = src.file_name() else { continue };
            let target = dir.join(name);
            let old = dir.join(format!("{}.old", name.to_string_lossy()));
            let step = (|| -> std::io::Result<()> {
                if target.exists() {
                    let _ = fs::remove_file(&old);
                    fs::rename(&target, &old)?;
                    swapped.push((target.clone(), old.clone()));
                }
                fs::copy(src, &target).map(|_| ())
            })();
            if let Err(e) = step {
                for (target, old) in swapped.iter().rev() {
                    let _ = fs::remove_file(target);
                    let _ = fs::rename(old, target);
                }
                return Err(format!("Could not replace {}: {e}", name.to_string_lossy()));
            }
        }
        progress(100.0);
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// Remove the `.old` files a previous update left beside the program.
pub fn cleanup() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "old") && p.is_file() {
            let _ = fs::remove_file(p);
        }
    }
}

/// `Glint.exe --after-update <pid>`: start up only when process `<pid>` (the copy that was just
/// replaced) has ended.
pub const AFTER_UPDATE: &str = "--after-update";
/// The longest the new copy waits for the old one to end.
const OLD_COPY_MAX_MS: u32 = 20_000;

/// The command that starts `exe` again after an update. A second copy started while this one is
/// alive would only hand over to it, so the new copy is told this process's id and waits for it
/// to end (`wait_for_old_copy`). Glint starts itself: no shell or script in between.
///
/// `minimized`: the new copy starts in the tray, with no window (an automatic update while the
/// window was closed). `recording`: whether clipping was on, so the new copy carries on the same
/// way instead of following "Start clipping when Glint starts".
fn relaunch_cmd(exe: &Path, minimized: bool, recording: Option<bool>) -> std::process::Command {
    let mut c = std::process::Command::new(exe);
    c.arg(AFTER_UPDATE).arg(std::process::id().to_string());
    if minimized {
        c.arg("--minimized");
    }
    match recording {
        Some(true) => {
            c.arg(REC_ON);
        }
        Some(false) => {
            c.arg(REC_OFF);
        }
        None => {}
    }
    if let Some(dir) = exe.parent() {
        c.current_dir(dir);
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c
}

/// Start `exe` again; it starts up once this process has gone.
pub fn relaunch(exe: &Path, minimized: bool, recording: Option<bool>) {
    if let Err(e) = relaunch_cmd(exe, minimized, recording).spawn() {
        util::log(&format!("update: could not start {}: {e}", exe.display()));
    }
}

/// `--rec-on` / `--rec-off`: given by the copy an update replaced, so clipping is on or off as it
/// was before the restart.
pub const REC_ON: &str = "--rec-on";
pub const REC_OFF: &str = "--rec-off";

/// Whether clipping must start (or stay off) whatever the setting says: Some when the copy an
/// update replaced said so.
pub fn rec_override(args: &[String]) -> Option<bool> {
    if args.iter().any(|a| a == REC_ON) {
        Some(true)
    } else if args.iter().any(|a| a == REC_OFF) {
        Some(false)
    } else {
        None
    }
}

/* ---------- automatic install ---------- */

/// Work that an automatic restart would destroy and that is not a job in the jobs list: an
/// export, a trim, an import, a component install. Hold a `Working` for as long as it runs.
static WORKING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub struct Working;
pub fn working() -> Working {
    WORKING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Working
}
impl Drop for Working {
    fn drop(&mut self) {
        WORKING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
pub fn working_now() -> usize {
    WORKING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Minutes in a row with nothing in the way before an update is installed by itself.
pub const AUTO_CALM_MINUTES: u32 = 3;

/// What an automatic install looks at.
#[derive(Clone, Debug, Default)]
pub struct AutoState {
    /// The setting "Install updates automatically".
    pub enabled: bool,
    /// A newer release is known.
    pub available: bool,
    /// Glint's window is open (the user may be in the middle of something).
    pub window_open: bool,
    /// A game that is running, if any (played now, or found among the running programs).
    pub game: Option<String>,
    /// Queued or running jobs (renders, compressions, joins).
    pub jobs: usize,
    /// Exports, trims, imports and installs under way (`working_now`).
    pub working: usize,
}

/// Why an update must not be installed (and Glint restarted) right now; None when it may.
/// A game always wins: never while one is running.
pub fn auto_blocker(s: &AutoState) -> Option<String> {
    if !s.enabled {
        return Some("automatic updates are off".into());
    }
    if !s.available {
        return Some("no update is waiting".into());
    }
    if let Some(game) = &s.game {
        return Some(format!("a game is running ({game})"));
    }
    if s.window_open {
        return Some("Glint's window is open".into());
    }
    if s.jobs > 0 {
        return Some(format!("{} job(s) in the renders list", s.jobs));
    }
    if s.working > 0 {
        return Some("an export, import or install is running".into());
    }
    None
}

/// Called first thing at start-up. With `--after-update <pid>` among `args`, wait until that
/// process has ended (at most 20 s), then a moment more so everything it held is released.
pub fn wait_for_old_copy(args: &[String]) {
    let Some(pid) = args.iter().position(|a| a == AFTER_UPDATE).and_then(|i| args.get(i + 1)).and_then(|p| p.parse::<u32>().ok()) else { return };
    if pid == 0 || pid == std::process::id() {
        return;
    }
    if !crate::win::wait_for_exit(pid, OLD_COPY_MAX_MS) {
        util::log(&format!("update: the old copy (process {pid}) did not end in {} s; starting anyway", OLD_COPY_MAX_MS / 1000));
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
}

/// `glint.exe --update-test <report.txt>`: headless checks. The network checks
/// run against the real repository; the swap is tested in the folder this exe
/// was copied to (never run this from the installed copy).
pub fn self_test(report_path: &str) {
    let mut lines: Vec<String> = Vec::new();
    if std::env::var_os("GLINT_DATA_DIR").is_none() {
        let _ = fs::write(report_path, "FAIL GLINT_DATA_DIR is not set\n");
        return;
    }
    let game_note;
    {
        let mut check = |name: &str, ok: bool| lines.push(format!("{} {name}", if ok { "PASS" } else { "FAIL" }));
        check("version order", newer("0.2.0", "0.1.9") && newer("v1.0.0", "0.9.9") && newer("0.1.10", "0.1.9") && !newer("0.1.0", "0.1.0") && !newer("0.1", "0.1.0") && !newer("", "0.1.0") && newer("1.2.3-beta", "1.2.2"));
        check("nothing is due right after a check", !due(&UpdateInfo { checked: util::unix_ms() / 1000, ..Default::default() }) && due(&UpdateInfo::default()));

        // The restart after an update: Glint starts itself and the new copy waits for the old one.
        let me = std::env::current_exe().unwrap_or_default();
        let cmd = relaunch_cmd(&me, false, None);
        let args_of = |c: &std::process::Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<String>>();
        let cmd_args = args_of(&cmd);
        check("the restart starts Glint.exe itself, with this process's id", cmd.get_program() == me.as_os_str() && cmd_args == [AFTER_UPDATE.to_string(), std::process::id().to_string()]);
        let pid = std::process::id().to_string();
        check("an automatic restart stays in the tray and keeps clipping on", args_of(&relaunch_cmd(&me, true, Some(true))) == [AFTER_UPDATE, pid.as_str(), "--minimized", REC_ON]);
        check("a restart while clipping was paused keeps it paused", args_of(&relaunch_cmd(&me, false, Some(false))) == [AFTER_UPDATE, pid.as_str(), REC_OFF]);
        let a = |list: &[&str]| list.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        check("the new copy reads whether clipping was on", rec_override(&a(&[AFTER_UPDATE, "1", REC_ON])) == Some(true) && rec_override(&a(&["--minimized", REC_OFF])) == Some(false) && rec_override(&a(&["--minimized"])).is_none());

        // Automatic install: only with the setting on, an update waiting, no game, the window closed and nothing running.
        let ready = AutoState { enabled: true, available: true, ..Default::default() };
        check("automatic install: goes ahead when nothing is in the way", auto_blocker(&ready).is_none());
        check("automatic install: not with the setting off", auto_blocker(&AutoState { enabled: false, ..ready.clone() }).is_some());
        check("automatic install: not without an update", auto_blocker(&AutoState { available: false, ..ready.clone() }).is_some());
        let in_game = auto_blocker(&AutoState { game: Some("Overwatch".into()), ..ready.clone() });
        check("automatic install: never while a game is running", in_game.as_deref().is_some_and(|w| w.contains("Overwatch")));
        check("automatic install: a game wins over everything else", auto_blocker(&AutoState { game: Some("osu!".into()), window_open: true, jobs: 2, working: 1, ..ready.clone() }).is_some_and(|w| w.contains("osu!")));
        check("automatic install: not while the window is open", auto_blocker(&AutoState { window_open: true, ..ready.clone() }).is_some());
        check("automatic install: not while a render or compression is queued", auto_blocker(&AutoState { jobs: 1, ..ready.clone() }).is_some());
        check("automatic install: not during an export or import", auto_blocker(&AutoState { working: 1, ..ready.clone() }).is_some());
        {
            let before = working_now();
            let w = working();
            let during = working_now();
            drop(w);
            check("work under way is counted and released", during == before + 1 && working_now() == before);
        }
        // Which running programs count as a game.
        let seen = [crate::games::Game { name: "My Indie Game".into(), exe: r"D:\Stuff\indie\Game.exe".into(), ..Default::default() }];
        let g = |paths: &[&str]| crate::games::game_among(&a(paths), &seen);
        check("a game from the table is found among the running programs", g(&[r"C:\Windows\explorer.exe", r"C:\Riot Games\VALORANT\live\ShooterGame\Binaries\Win64\VALORANT-Win64-Shipping.exe"]).as_deref() == Some("Valorant"));
        check("a game Glint has seen being played is found by its program", g(&[r"d:\stuff\indie\game.exe"]).as_deref() == Some("My Indie Game"));
        check("a program inside a game store's folder counts as a game", g(&[r"E:\SteamLibrary\steamapps\common\Some New Game\SomeNewGame.exe"]).is_some());
        check("launchers, helpers and everyday programs are not games", g(&[r"C:\Program Files (x86)\Steam\steam.exe", r"C:\Program Files\Epic Games\Launcher\Engine\Binaries\Win64\EpicWebHelper.exe", r"C:\Program Files\Epic Games\Launcher\Portal\Binaries\Win64\EpicGamesLauncher.exe", r"C:\Users\x\AppData\Local\Discord\app-1.0\Discord.exe", r"C:\Program Files\Mozilla Firefox\firefox.exe", r"E:\SteamLibrary\steamapps\common\Some New Game\UnityCrashHandler64.exe", r"C:\Program Files\Java\bin\javaw.exe"]).is_none());
        check("Minecraft in Java is found by its folder", g(&[r"C:\Users\x\AppData\Roaming\.minecraft\runtime\java-runtime\bin\javaw.exe"]).as_deref() == Some("Minecraft"));
        check("no programs, no game", g(&[]).is_none());
        // This PC, right now (read only): the list must come back, and what counts as a game is reported.
        let started = std::time::Instant::now();
        let procs = crate::win::process_paths();
        let now = crate::games::game_among(&procs, &[]);
        check(&format!("the running programs can be listed ({} programs in {} ms)", procs.len(), started.elapsed().as_millis()), procs.len() > 10 && procs.iter().any(|p| p.to_lowercase().ends_with("explorer.exe")));
        game_note = format!("INFO a game running on this PC now: {}", now.as_deref().unwrap_or("none"));
        check("the restart runs in Glint's folder", cmd.get_current_dir() == me.parent());
        // A stand-in for the old copy: a hidden process that ends by itself after about 2 s.
        let sleeper = || util::hidden_cmd(Path::new("cmd.exe")).args(["/c", "ping -n 3 127.0.0.1 >nul"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        match sleeper() {
            Ok(mut old) => {
                check("a running process is not reported as ended", !crate::win::wait_for_exit(old.id(), 100) && crate::win::process_alive(old.id()));
                // The real start-up path: `--after-update <pid>` first, then a headless test that writes a report.
                let probe = PathBuf::from(format!("{report_path}.after-update.txt"));
                let _ = fs::remove_file(&probe);
                let started = std::time::Instant::now();
                let ran = std::process::Command::new(&me).arg(AFTER_UPDATE).arg(old.id().to_string()).arg("--memory-test").arg(&probe).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
                let waited = started.elapsed().as_millis();
                let old_gone = !crate::win::process_alive(old.id());
                check(&format!("the new copy started only after the old one ended (waited {waited} ms)"), ran && old_gone && probe.is_file() && waited >= 1000);
                let _ = old.kill();
                let _ = old.wait();
                let _ = fs::remove_file(&probe);
            }
            Err(e) => check(&format!("a stand-in process could be started ({e})"), false),
        }
        let started = std::time::Instant::now();
        wait_for_old_copy(&[AFTER_UPDATE.to_string(), "4294967290".to_string()]);
        wait_for_old_copy(&[AFTER_UPDATE.to_string(), std::process::id().to_string()]);
        wait_for_old_copy(&[AFTER_UPDATE.to_string()]);
        wait_for_old_copy(&["--minimized".to_string()]);
        check("no wait for a process that is gone, for itself, or without an id", started.elapsed().as_millis() < 2000);
    }
    lines.push(game_note);
    let info = check();
    lines.push(format!("INFO check: current={} latest={:?} available={} url={} sha={} error={:?}", info.current, info.latest, info.available, !info.url.is_empty(), !info.sha_url.is_empty(), info.error));
    lines.push(format!("{} the check reached GitHub", if info.error.is_empty() { "PASS" } else { "FAIL" }));
    lines.push(format!("{} the state was saved", if load().checked == info.checked && info.checked > 0 { "PASS" } else { "FAIL" }));
    // With a release published, install it over this copy and verify the swap.
    if !info.url.is_empty() && std::env::var_os("GLINT_UPDATE_TEST_INSTALL").is_some() {
        let exe = std::env::current_exe().unwrap_or_default();
        let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        let before = fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
        let mut last = 0f32;
        let mut steady = true;
        let res = install_into(&info, &dir, &mut |p| {
            steady &= p >= last;
            last = p;
        });
        lines.push(format!("{} install: {res:?}", if res.is_ok() { "PASS" } else { "FAIL" }));
        lines.push(format!("{} progress rose to 100", if steady && last >= 100.0 { "PASS" } else { "FAIL" }));
        let old = dir.join(format!("{}.old", exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        lines.push(format!("{} the running exe was kept as .old ({} bytes)", if fs::metadata(&old).map(|m| m.len()).unwrap_or(0) == before && before > 0 { "PASS" } else { "FAIL" }, before));
        lines.push(format!("{} a new Glint.exe is in place", if fs::metadata(dir.join("Glint.exe")).map(|m| m.len() > 1_000_000).unwrap_or(false) { "PASS" } else { "FAIL" }));
        lines.push(format!("{} WebView2Loader.dll is in place", if dir.join("WebView2Loader.dll").is_file() { "PASS" } else { "FAIL" }));
    }
    let _ = fs::write(report_path, lines.join("\n") + "\n");
}
