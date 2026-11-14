//! GlintSetup.exe: the one-time installer, a small Windows setup wizard.
//!
//! It downloads everything from GitHub releases and puts it in place:
//!   - Glint itself          github.com/swamie/glint          (latest release)
//!   - ffmpeg and ffprobe    github.com/GyanD/codexffmpeg     (latest release, "essentials" build)
//!   - Blur (optional)       github.com/f0e/blur              (v2.23, the version Glint's settings match)
//! and, only if this PC lacks it, Microsoft's WebView2 runtime from microsoft.com
//! (Windows 11 has it built in).
//!
//! After this, Glint updates itself from its GitHub releases (see update.rs).
//! The window is plain Win32 (setup_ui.rs). Downloads and unpacking use curl.exe and
//! tar.exe (part of Windows 10 1803 and newer), started directly without a console.
//! No PowerShell, cmd or scripts anywhere: shortcuts are made through COM (IShellLink),
//! and the uninstaller removes its own folder through a copy of itself in %TEMP%.
//!
//! Options (all optional; with none it opens the wizard):
//!   --dir <folder>     install here instead of %LOCALAPPDATA%\Programs\Glint
//!   --yes              no window at all, take the default for every choice (logs to %TEMP%\GlintSetup.log)
//!   --no-ffmpeg        do not fetch ffmpeg        --force-ffmpeg  fetch it even if one is on PATH
//!   --no-blur          do not fetch Blur
//!   --no-shortcuts     no Start menu or desktop shortcut, no entry under "Installed apps"
//!   --no-launch        do not start Glint at the end
//!   --uninstall        remove Glint (clips and settings are kept unless you say otherwise)
//! For tests:
//!   --test-root <folder>   shortcuts, settings folders and registry entries go to a test place
//!                          (<folder>\Start menu, <folder>\Desktop, HKCU\Software\Glint-setup-test)
//!   --ui-test <folder>     build every page hidden, draw each into a PNG, write report.txt
//!   --cancel-test <folder> a real install into <folder>, cancelled after 1 MB: must leave nothing
//! Internal: --remove-files <folder> --wait <pid> [--files a|b]  (the uninstaller's copy in %TEMP%)
#![windows_subsystem = "windows"]

mod setup_ui;

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const REPO: &str = "swamie/glint";
const ASSET: &str = "Glint-windows-x64.zip";
const FFMPEG_REPO: &str = "GyanD/codexffmpeg";
const BLUR_REPO: &str = "f0e/blur";
pub const BLUR_TAG: &str = "v2.23";
const BLUR_ASSET: &str = "blur-Windows-Installer-x64.exe";
const WEBVIEW2_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Glint";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const TEST_KEY: &str = r"Software\Glint-setup-test";
/// The files this installer puts in the program folder (the uninstaller removes these, never anything else).
const KNOWN_FILES: [&str; 5] = ["Glint.exe", "WebView2Loader.dll", "ffmpeg.exe", "ffprobe.exe", "GlintSetup.exe"];
pub const CANCELLED: &str = "You cancelled the installation. Nothing was changed.";
/// CREATE_NO_WINDOW: console programs (curl, tar, ffmpeg) get no console window.
const NO_WINDOW: u32 = 0x0800_0000;

/// Where shortcuts, data folders and registry entries live. `test()` moves all of them into a test folder
/// and a test registry key, so a test never touches the real Start menu, Desktop or "Installed apps".
#[derive(Clone, Debug)]
pub struct Places {
    pub start_menu_link: PathBuf,
    pub desktop_link: PathBuf,
    pub uninstall_key: String,
    pub run_key: String,
    pub app_data: PathBuf,
    pub local_data: PathBuf,
    pub test: bool,
}

impl Places {
    pub fn real() -> Places {
        use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Programs};
        Places {
            start_menu_link: known_folder(&FOLDERID_Programs).unwrap_or_else(|| env_dir("APPDATA").join(r"Microsoft\Windows\Start Menu\Programs")).join("Glint.lnk"),
            desktop_link: known_folder(&FOLDERID_Desktop).unwrap_or_else(|| env_dir("USERPROFILE").join("Desktop")).join("Glint.lnk"),
            uninstall_key: UNINSTALL_KEY.into(),
            run_key: RUN_KEY.into(),
            app_data: env_dir("APPDATA").join("Glint"),
            local_data: env_dir("LOCALAPPDATA").join("Glint"),
            test: false,
        }
    }
    pub fn test(root: &Path) -> Places {
        Places {
            start_menu_link: root.join(r"Start menu\Glint.lnk"),
            desktop_link: root.join(r"Desktop\Glint.lnk"),
            uninstall_key: format!(r"{TEST_KEY}\Uninstall\Glint"),
            run_key: format!(r"{TEST_KEY}\Run"),
            app_data: root.join(r"AppData\Glint"),
            local_data: root.join(r"LocalAppData\Glint"),
            test: true,
        }
    }
    /// Where Blur is installed (and where Glint looks for it).
    pub fn blur_dir(&self) -> PathBuf {
        self.local_data.join("blur")
    }
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub dir: PathBuf,
    pub yes: bool,
    /// Fetch ffmpeg when this PC needs it.
    pub ffmpeg: bool,
    /// Fetch ffmpeg even when a suitable one is already there.
    pub force_ffmpeg: bool,
    pub blur: bool,
    pub start_menu: bool,
    pub desktop: bool,
    /// The entry under "Installed apps" and the copy of this program beside Glint (the uninstaller).
    pub register: bool,
    pub launch: bool,
    /// Uninstall: also remove the settings, the clip list and Blur.
    pub purge: bool,
    pub places: Places,
}

fn env_dir(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, windows::Win32::Foundation::HANDLE::default()).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

// ---------------------------------------------------------------- log

static LOG: OnceLock<Mutex<Option<fs::File>>> = OnceLock::new();

pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("GlintSetup.log")
}

fn log_open() {
    let path = log_path();
    if fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = fs::remove_file(&path);
    }
    let f = fs::OpenOptions::new().create(true).append(true).open(&path).ok();
    let _ = LOG.set(Mutex::new(f));
}

/// One line to %TEMP%\GlintSetup.log (and to stdout, when there is one).
pub fn log(text: &str) {
    let _ = writeln!(std::io::stdout(), "{text}");
    if let Some(m) = LOG.get() {
        if let Ok(mut g) = m.lock() {
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "{text}");
            }
        }
    }
}

// ---------------------------------------------------------------- small helpers

/// A console program started without a console window.
pub fn hidden(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(NO_WINDOW).stdin(Stdio::null());
    c
}

/// GET a GitHub API address and parse the JSON.
fn api(path: &str) -> Result<serde_json::Value, String> {
    let out = hidden("curl.exe")
        .args(["-sS", "-L", "--fail", "--retry", "2", "--max-time", "30", "-H", "User-Agent: Glint-setup", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{path}"))
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("curl.exe could not be started ({e}). Glint needs Windows 10 version 1803 or newer."))?;
    if !out.status.success() {
        log(&format!("GitHub API {path}: {}", String::from_utf8_lossy(&out.stderr).trim()));
        return Err("GitHub could not be reached. Check your internet connection, then try again.".into());
    }
    serde_json::from_slice(&out.stdout).map_err(|_| format!("GitHub sent an unreadable answer for {path}."))
}

/// (name, download address, size) of the first asset of a release whose name passes `want`.
fn asset(release: &serde_json::Value, want: impl Fn(&str) -> bool) -> Option<(String, String, u64)> {
    release["assets"].as_array()?.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        want(name).then(|| (name.to_string(), a["browser_download_url"].as_str().unwrap_or("").to_string(), a["size"].as_u64().unwrap_or(0)))
    })
}

pub fn mb(bytes: u64) -> String {
    let m = bytes as f64 / 1e6;
    if m < 10.0 { format!("{m:.1} MB") } else { format!("{m:.0} MB") }
}

/// Unpack a zip (tar.exe on Windows reads zip files). `only` limits it to matching paths.
fn unzip(zip: &Path, into: &Path, only: &[&str]) -> bool {
    let _ = fs::create_dir_all(into);
    hidden("tar.exe").arg("-xf").arg(zip).arg("-C").arg(into).args(only).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn find_file(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_file() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
            return Some(p);
        }
        if p.is_dir() && depth > 0 {
            if let Some(found) = find_file(&p, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    norm(a) == norm(b)
}

fn inside(path: &Path, dir: &Path) -> bool {
    let d = dir.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    let p = path.to_string_lossy().replace('/', "\\").to_lowercase();
    p.starts_with(&format!("{d}\\"))
}

/// True if the WebView2 runtime (which draws Glint's window) is installed.
pub fn has_webview2() -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    const ID: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let found = |root: RegKey, base: &str| root.open_subkey(format!(r"{base}\{ID}")).and_then(|k| k.get_value::<String, _>("pv")).map(|v| !v.is_empty() && v != "0.0.0.0").unwrap_or(false);
    found(RegKey::predef(HKEY_LOCAL_MACHINE), r"SOFTWARE\WOW6432Node")
        || found(RegKey::predef(HKEY_LOCAL_MACHINE), "SOFTWARE")
        || found(RegKey::predef(HKEY_CURRENT_USER), "Software")
}

/// True if an ffmpeg on PATH has the screen-capture filter Glint uses (ffmpeg 8 or newer).
pub fn path_ffmpeg_ok() -> bool {
    hidden("ffmpeg.exe")
        .args(["-hide_banner", "-filters"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(" gfxcapture "))
        .unwrap_or(false)
}

pub fn ffmpeg_in(dir: &Path) -> bool {
    dir.join("ffmpeg.exe").is_file()
}

pub fn blur_installed(p: &Places) -> bool {
    p.blur_dir().join("blur-cli.exe").is_file()
}

/// Free bytes on the drive that holds `dir` (or its nearest existing parent).
pub fn free_space(dir: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = dir.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&windows::core::HSTRING::from(p.as_os_str()), Some(&mut free), None, None).ok()? };
    Some(free)
}

/// True if a Glint.exe started from `dir` is running.
pub fn glint_running(dir: &Path) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    let want = dir.join("Glint.exe");
    let mut found = false;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok && !found {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("glint.exe") {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, e.th32ProcessID) {
                    let mut buf = [0u16; 1024];
                    let mut n = buf.len() as u32;
                    if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut n).is_ok() {
                        found = same_path(Path::new(&String::from_utf16_lossy(&buf[..n as usize])), &want);
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    found
}

/// Wait up to `ms` for process `pid` to end. True if it ended (or was not running).
fn wait_pid(pid: u32, ms: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) else { return true };
        let r = WaitForSingleObject(h, ms);
        let _ = CloseHandle(h);
        r == WAIT_OBJECT_0
    }
}

// ---------------------------------------------------------------- shortcuts (COM, no shell)

fn hs(p: &Path) -> windows::core::HSTRING {
    windows::core::HSTRING::from(p.as_os_str())
}

/// Make a .lnk at `link` that starts `target`.
pub fn make_link(link: &Path, target: &Path) -> Result<(), String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let e = |e: windows::core::Error| format!("The shortcut {} could not be made ({})", link.display(), e.message());
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(e)?;
        sl.SetPath(&hs(target)).map_err(e)?;
        if let Some(d) = target.parent() {
            sl.SetWorkingDirectory(&hs(d)).map_err(e)?;
        }
        sl.SetDescription(windows::core::w!("Glint: instant replays of your games")).map_err(e)?;
        sl.SetIconLocation(&hs(target), 0).map_err(e)?;
        let pf: IPersistFile = sl.cast().map_err(e)?;
        if let Some(p) = link.parent() {
            let _ = fs::create_dir_all(p);
        }
        pf.Save(&hs(link), true).map_err(e)?;
    }
    Ok(())
}

/// (target, working folder, description) of a .lnk.
pub fn read_link(link: &Path) -> Option<(PathBuf, PathBuf, String)> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = sl.cast().ok()?;
        pf.Load(&hs(link), STGM_READ).ok()?;
        let mut path = [0u16; 1024];
        sl.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
        let mut dir = [0u16; 1024];
        let _ = sl.GetWorkingDirectory(&mut dir);
        let mut desc = [0u16; 512];
        let _ = sl.GetDescription(&mut desc);
        Some((PathBuf::from(text(&path)), PathBuf::from(text(&dir)), text(&desc)))
    }
}

// ---------------------------------------------------------------- what the wizard shows before installing

/// What Setup can find out before it starts: sizes, what is already on this PC.
#[derive(Clone, Debug, Default)]
pub struct Probe {
    pub done: bool,
    pub version: String,
    pub glint_size: u64,
    pub ffmpeg_size: u64,
    pub blur_size: u64,
    pub ffmpeg_on_path: bool,
    pub webview2: bool,
    pub blur_installed: bool,
}

pub fn probe(o: &Opts) -> Probe {
    let mut p = Probe { ffmpeg_on_path: path_ffmpeg_ok(), webview2: has_webview2(), blur_installed: blur_installed(&o.places), ..Default::default() };
    if let Ok(r) = api(&format!("{REPO}/releases/latest")) {
        p.version = r["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
        p.glint_size = asset(&r, |n| n == ASSET).map(|a| a.2).unwrap_or(0);
    }
    if let Ok(r) = api(&format!("{FFMPEG_REPO}/releases/latest")) {
        p.ffmpeg_size = asset(&r, |n| n.ends_with("essentials_build.zip")).map(|a| a.2).unwrap_or(0);
    }
    if !p.blur_installed {
        if let Ok(r) = api(&format!("{BLUR_REPO}/releases/tags/{BLUR_TAG}")) {
            p.blur_size = asset(&r, |n| n == BLUR_ASSET).map(|a| a.2).unwrap_or(0);
        }
    }
    p.done = true;
    p
}

// ---------------------------------------------------------------- install

/// Where the install reports to: the wizard's progress page, or the log with --yes.
pub trait Sink: Sync {
    /// A new step: `index` of `total` (both 0 while the plan is not known yet).
    fn step(&self, text: &str, index: usize, total: usize);
    /// Download progress. `total` 0 = unknown.
    fn bytes(&self, done: u64, total: u64, per_sec: f64);
    /// Work without a known length (unpacking, a program's own installer).
    fn busy(&self, text: &str);
    /// Whether Cancel can still stop it (false once files are being put in place).
    fn can_cancel(&self, yes: bool);
    fn cancelled(&self) -> bool;
}

#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub version: String,
    pub exe: PathBuf,
    /// Things the user should know (a part that did not install, Glint still running).
    pub notes: Vec<String>,
}

/// Download with curl, reporting the size of the growing file. Cancel kills curl and deletes the part.
fn download(url: &str, to: &Path, expected: u64, s: &dyn Sink) -> Result<(), String> {
    let _ = fs::remove_file(to);
    let mut child = hidden("curl.exe")
        .args(["-sS", "-L", "--fail", "--retry", "2", "--connect-timeout", "20", "--speed-limit", "1", "--speed-time", "60", "-H", "User-Agent: Glint-setup", "-o"])
        .arg(to)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl.exe could not be started ({e}). Glint needs Windows 10 version 1803 or newer."))?;
    let start = Instant::now();
    let mut window: Vec<(Instant, u64)> = vec![(start, 0)];
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            let size = fs::metadata(to).map(|m| m.len()).unwrap_or(0);
            if status.success() && to.is_file() {
                s.bytes(size, expected.max(size), 0.0);
                log(&format!("      downloaded {} in {:.1} s", mb(size), start.elapsed().as_secs_f64()));
                return Ok(());
            }
            log(&format!("      curl failed for {url}: {}", err.trim()));
            let _ = fs::remove_file(to);
            return Err(format!("The download failed ({}). Check your internet connection, then try again.", err.trim().trim_start_matches("curl: ").chars().take(160).collect::<String>()));
        }
        if s.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(to);
            return Err(CANCELLED.into());
        }
        let size = fs::metadata(to).map(|m| m.len()).unwrap_or(0);
        let now = Instant::now();
        window.push((now, size));
        window.retain(|(t, _)| now.duration_since(*t) < Duration::from_millis(1500));
        let (t0, b0) = window[0];
        let secs = now.duration_since(t0).as_secs_f64();
        let speed = if secs > 0.2 { (size.saturating_sub(b0)) as f64 / secs } else { 0.0 };
        s.bytes(size, expected, speed);
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Puts files in place so that a failure can undo all of it: an existing file is renamed to `.old`
/// first (a running exe cannot be overwritten on Windows but can be renamed).
#[derive(Default)]
struct Txn {
    done: Vec<(PathBuf, Option<PathBuf>)>,
}

impl Txn {
    fn put(&mut self, src: &Path, dst: &Path) -> Result<(), String> {
        let backup = if dst.exists() {
            let old = PathBuf::from(format!("{}.old", dst.display()));
            let _ = fs::remove_file(&old);
            fs::rename(dst, &old).map_err(|e| format!("Could not replace {}: {e}", dst.display()))?;
            Some(old)
        } else {
            None
        };
        match fs::copy(src, dst) {
            Ok(_) => {
                self.done.push((dst.to_path_buf(), backup));
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(dst);
                if let Some(b) = backup {
                    let _ = fs::rename(b, dst);
                }
                Err(format!("Could not write {}: {e}", dst.display()))
            }
        }
    }
    fn rollback(&mut self) {
        for (dst, backup) in self.done.drain(..).rev() {
            let _ = fs::remove_file(&dst);
            if let Some(b) = backup {
                let _ = fs::rename(&b, &dst);
            }
        }
    }
    fn finish(self) {
        for (_, backup) in self.done {
            if let Some(b) = backup {
                let _ = fs::remove_file(b); // fails harmlessly while the old file is still running
            }
        }
    }
}

/// Install Glint. Everything is downloaded and unpacked into `tmp` first; only then are files put in
/// place, and a failure there puts the old files back. So a failed or cancelled install changes nothing.
pub fn install(o: &Opts, tmp: &Path, s: &dyn Sink) -> Result<Outcome, String> {
    let check = || if s.cancelled() { Err(CANCELLED.to_string()) } else { Ok(()) };
    fs::create_dir_all(tmp).map_err(|e| format!("Setup could not make a temporary folder: {e}"))?;
    let mut notes = Vec::new();
    s.can_cancel(true);

    // The plan.
    s.step("Looking for the latest Glint", 0, 0);
    s.busy("Asking GitHub for the latest release");
    log("[plan]");
    let release = api(&format!("{REPO}/releases/latest"))?;
    check()?;
    let version = release["tag_name"].as_str().unwrap_or("?").trim_start_matches('v').to_string();
    let (name, url, size) = asset(&release, |n| n == ASSET).ok_or(format!("Glint {version} on GitHub has no {ASSET}. Try again in a few minutes."))?;
    let ff_fetch = o.ffmpeg && (o.force_ffmpeg || !(ffmpeg_in(&o.dir) || path_ffmpeg_ok()));
    let wv_fetch = !has_webview2();
    let blur_fetch = o.blur && !blur_installed(&o.places);
    let total = 2 + ff_fetch as usize + wv_fetch as usize + blur_fetch as usize;
    let mut n = 1;
    log(&format!("      Glint {version}; ffmpeg {}; WebView2 {}; Blur {}", if ff_fetch { "download" } else { "no" }, if wv_fetch { "download" } else { "present" }, if blur_fetch { "download" } else { "no" }));

    // 1. Glint
    s.step(&format!("Downloading Glint {version}"), n, total);
    log(&format!("[{n}/{total}] Glint {version} ({})", mb(size)));
    let zip = tmp.join(&name);
    download(&url, &zip, size, s)?;
    s.busy("Unpacking Glint");
    let unpacked = tmp.join("glint");
    if !unzip(&zip, &unpacked, &[]) || !unpacked.join("Glint.exe").is_file() {
        return Err("Glint's download could not be unpacked. Try again.".into());
    }
    check()?;

    // 2. ffmpeg
    let mut ff_files = Vec::new();
    if ff_fetch {
        n += 1;
        s.step("Downloading ffmpeg", n, total);
        s.busy("Asking GitHub for ffmpeg");
        let release = api(&format!("{FFMPEG_REPO}/releases/latest"))?;
        let (name, url, size) = asset(&release, |n| n.ends_with("essentials_build.zip")).ok_or("No ffmpeg build was found on GitHub. Try again later, or untick ffmpeg.")?;
        log(&format!("[{n}/{total}] {name} ({})", mb(size)));
        s.step(&format!("Downloading ffmpeg ({})", mb(size)), n, total);
        let zip = tmp.join(&name);
        download(&url, &zip, size, s)?;
        s.busy("Unpacking ffmpeg");
        let unpacked = tmp.join("ffmpeg");
        // Only the two programs are needed out of the archive.
        if !unzip(&zip, &unpacked, &["*/bin/ffmpeg.exe", "*/bin/ffprobe.exe"]) {
            unzip(&zip, &unpacked, &[]);
        }
        for exe in ["ffmpeg.exe", "ffprobe.exe"] {
            ff_files.push((find_file(&unpacked, exe, 4).ok_or(format!("{exe} was not in the ffmpeg download. Try again."))?, exe));
        }
        check()?;
    }

    // 3. WebView2 (draws the window). Part of Windows 11; older systems may lack it.
    let mut wv_exe = None;
    if wv_fetch {
        n += 1;
        s.step("Downloading the WebView2 runtime", n, total);
        log(&format!("[{n}/{total}] WebView2 runtime"));
        let exe = tmp.join("MicrosoftEdgeWebview2Setup.exe");
        download(WEBVIEW2_URL, &exe, 0, s)?;
        wv_exe = Some(exe);
        check()?;
    }

    // 4. Blur (optional)
    let mut blur_exe = None;
    if blur_fetch {
        n += 1;
        s.step(&format!("Downloading Blur {BLUR_TAG}"), n, total);
        s.busy("Asking GitHub for Blur");
        let release = api(&format!("{BLUR_REPO}/releases/tags/{BLUR_TAG}"))?;
        match asset(&release, |n| n == BLUR_ASSET) {
            None => notes.push("Blur's download was not found. You can add it later in Glint under Settings, Components.".to_string()),
            Some((name, url, size)) => {
                log(&format!("[{n}/{total}] Blur {BLUR_TAG} ({})", mb(size)));
                let exe = tmp.join(&name);
                download(&url, &exe, size, s)?;
                blur_exe = Some(exe);
            }
        }
        check()?;
    }

    // 5. Put it all in place. From here on Cancel waits: this part is short and is undone on failure.
    n += 1;
    s.can_cancel(false);
    s.step("Installing", n, total);
    s.busy("Copying files");
    log(&format!("[{n}/{total}] install into {}", o.dir.display()));
    fs::create_dir_all(&o.dir).map_err(|e| format!("Setup could not make the folder {} ({e}). Choose another folder.", o.dir.display()))?;
    let was_running = glint_running(&o.dir);
    let mut tx = Txn::default();
    let mut files: Vec<String> = Vec::new();
    let copied = (|| -> Result<(), String> {
        for e in fs::read_dir(&unpacked).map_err(|e| e.to_string())?.flatten().filter(|e| e.path().is_file()) {
            tx.put(&e.path(), &o.dir.join(e.file_name()))?;
            files.push(e.file_name().to_string_lossy().to_string());
        }
        for (src, name) in &ff_files {
            tx.put(src, &o.dir.join(name))?;
            files.push(name.to_string());
        }
        if o.register {
            // A copy of this installer beside Glint: it is also the uninstaller.
            let me = std::env::current_exe().map_err(|e| e.to_string())?;
            let copy = o.dir.join("GlintSetup.exe");
            if !same_path(&me, &copy) {
                tx.put(&me, &copy)?;
            }
            files.push("GlintSetup.exe".into());
        }
        Ok(())
    })();
    if let Err(e) = copied {
        tx.rollback();
        log(&format!("      failed, put the old files back: {e}"));
        return Err(e);
    }
    tx.finish();
    let exe = o.dir.join("Glint.exe");
    if was_running {
        notes.push("Glint was running. Quit it from its tray icon and start it again to use the new version.".into());
    }

    // Separate programs with their own installers: a failure here is reported, not undone.
    if let Some(wv) = wv_exe {
        s.busy("Installing the WebView2 runtime (Microsoft's installer downloads the rest)");
        let ok = hidden(&wv).args(["/silent", "/install"]).status().map(|s| s.success()).unwrap_or(false);
        log(&format!("      WebView2: {}", if ok { "installed" } else { "failed" }));
        if !ok {
            notes.push("The WebView2 runtime did not install. Get \"WebView2 Runtime\" from microsoft.com, then start Glint.".into());
        }
    }
    if let Some(blur) = blur_exe {
        s.busy("Installing Blur (Windows may ask for permission)");
        let dir = o.places.blur_dir();
        // Blur ships as an Inno Setup installer: these are Inno's standard unattended switches.
        let _ = hidden(&blur).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CURRENTUSER"]).arg(format!("/DIR={}", dir.display())).status();
        let ok = dir.join("blur-cli.exe").is_file();
        log(&format!("      Blur: {}", if ok { "installed" } else { "not found after its installer" }));
        if !ok {
            notes.push("Blur did not end up where Glint looks for it. Add it later in Glint under Settings, Components. Everything else works without it.".into());
        }
    }

    // Shortcuts and the entry under "Installed apps".
    if o.start_menu || o.desktop {
        s.busy("Adding shortcuts");
    }
    for (want, link, what) in [(o.start_menu, &o.places.start_menu_link, "Start menu"), (o.desktop, &o.places.desktop_link, "desktop")] {
        if want {
            match make_link(link, &exe) {
                Ok(()) => log(&format!("      {what} shortcut: {}", link.display())),
                Err(e) => {
                    log(&format!("      {e}"));
                    notes.push(format!("The {what} shortcut could not be made."));
                }
            }
        }
    }
    if o.register {
        register(o, &version, &files);
    }
    log("Done.");
    Ok(Outcome { version, exe, notes })
}

fn register(o: &Opts, version: &str, files: &[String]) {
    use winreg::enums::HKEY_CURRENT_USER;
    let copy = o.dir.join("GlintSetup.exe");
    let kb: u64 = files.iter().filter_map(|f| fs::metadata(o.dir.join(f)).ok()).map(|m| m.len()).sum::<u64>() / 1024;
    match winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(&o.places.uninstall_key) {
        Ok((key, _)) => {
            let _ = key.set_value("DisplayName", &"Glint");
            let _ = key.set_value("DisplayVersion", &version);
            let _ = key.set_value("Publisher", &"Glint");
            let _ = key.set_value("DisplayIcon", &o.dir.join("Glint.exe").to_string_lossy().to_string());
            let _ = key.set_value("InstallLocation", &o.dir.to_string_lossy().to_string());
            let _ = key.set_value("UninstallString", &format!("\"{}\" --uninstall", copy.display()));
            let _ = key.set_value("QuietUninstallString", &format!("\"{}\" --uninstall --yes", copy.display()));
            let _ = key.set_value("URLInfoAbout", &format!("https://github.com/{REPO}"));
            let _ = key.set_value("EstimatedSize", &(kb.min(u32::MAX as u64) as u32));
            let _ = key.set_value("NoModify", &1u32);
            let _ = key.set_value("NoRepair", &1u32);
            // What the uninstaller may remove from the program folder.
            let _ = key.set_value("GlintFiles", &files.join("|"));
            log(&format!("      registered under HKCU\\{}", o.places.uninstall_key));
        }
        Err(e) => log(&format!("      could not write HKCU\\{}: {e}", o.places.uninstall_key)),
    }
}

/// Start the installed Glint (directly, not through a shell).
pub fn launch(exe: &Path) {
    let r = Command::new(exe).current_dir(exe.parent().unwrap_or(Path::new("."))).spawn();
    log(&format!("Start Glint: {}", if r.is_ok() { "started" } else { "failed" }));
}

// ---------------------------------------------------------------- uninstall

#[derive(Clone, Debug, Default)]
pub struct Removal {
    /// What was removed, in plain words.
    pub done: Vec<String>,
    /// What could not be removed.
    pub problems: Vec<String>,
    /// The program files go once this program exits (it runs from that folder).
    pub files_later: bool,
    pub files: Vec<String>,
}

/// The names the uninstaller may remove from the program folder: the fixed list plus what the
/// install recorded.
fn files_to_remove(o: &Opts) -> Vec<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    let mut names: Vec<String> = KNOWN_FILES.iter().map(|s| s.to_string()).collect();
    if let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(&o.places.uninstall_key) {
        let loc: String = key.get_value("InstallLocation").unwrap_or_default();
        if same_path(Path::new(&loc), &o.dir) {
            let recorded: String = key.get_value("GlintFiles").unwrap_or_default();
            for f in recorded.split('|').map(str::trim).filter(|f| !f.is_empty() && !f.contains(['\\', '/', ':']) && f != &".." && f != &".") {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(f)) {
                    names.push(f.to_string());
                }
            }
        }
    }
    names
}

/// Remove Glint. Only what points at `o.dir` is touched: a shortcut, a "start with Windows" entry or an
/// uninstall entry that belongs to another copy of Glint stays.
pub fn uninstall(o: &Opts) -> Removal {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    log(&format!("[uninstall] {}", o.dir.display()));
    let mut r = Removal { files: files_to_remove(o), ..Default::default() };
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    for (link, what) in [(&o.places.start_menu_link, "the Start menu shortcut"), (&o.places.desktop_link, "the desktop shortcut")] {
        if link.is_file() {
            let ours = read_link(link).map(|(t, _, _)| inside(&t, &o.dir)).unwrap_or(false);
            if !ours {
                log(&format!("      kept {} (it points somewhere else)", link.display()));
            } else if fs::remove_file(link).is_ok() {
                r.done.push(format!("Removed {what}."));
            } else {
                r.problems.push(format!("Could not remove {what}."));
            }
        }
    }
    if let Ok(run) = hkcu.open_subkey_with_flags(&o.places.run_key, KEY_READ | KEY_SET_VALUE) {
        let v: String = run.get_value("Glint").unwrap_or_default();
        if !v.is_empty() && v.to_lowercase().contains(&o.dir.to_string_lossy().trim_end_matches('\\').to_lowercase()) && run.delete_value("Glint").is_ok() {
            r.done.push("Glint no longer starts with Windows.".into());
        }
    }
    if let Ok(key) = hkcu.open_subkey(&o.places.uninstall_key) {
        let loc: String = key.get_value("InstallLocation").unwrap_or_default();
        drop(key);
        if loc.is_empty() || same_path(Path::new(&loc), &o.dir) {
            if hkcu.delete_subkey_all(&o.places.uninstall_key).is_ok() {
                r.done.push("Removed Glint from Installed apps.".into());
            }
        }
    }
    if o.purge {
        for d in [&o.places.app_data, &o.places.local_data] {
            if d.exists() {
                match fs::remove_dir_all(d) {
                    Ok(()) => r.done.push(format!("Removed {}.", d.display())),
                    Err(e) => r.problems.push(format!("Could not remove all of {} ({e}).", d.display())),
                }
            }
        }
    }
    let me = std::env::current_exe().unwrap_or_default();
    if inside(&me, &o.dir) {
        r.files_later = true;
        r.done.push(format!("Glint's files in {} go as soon as this window closes.", o.dir.display()));
    } else {
        let left = remove_files(&o.dir, &r.files);
        if left.is_empty() {
            r.done.push(format!("Removed Glint from {}.", o.dir.display()));
        } else {
            r.problems.push(format!("Could not remove {} (is Glint still running?).", left.join(", ")));
        }
    }
    for line in r.done.iter().chain(r.problems.iter()) {
        log(&format!("      {line}"));
    }
    r
}

/// Remove the named files (and their `.old` copies) from `dir`, then `dir` itself if that leaves it
/// empty. Nothing else in the folder is touched, so a wrong folder cannot be wiped. Returns what is left.
pub fn remove_files(dir: &Path, names: &[String]) -> Vec<String> {
    let mut left = Vec::new();
    for name in names {
        for p in [dir.join(name), dir.join(format!("{name}.old"))] {
            if !p.exists() {
                continue;
            }
            let mut ok = false;
            for _ in 0..10 {
                if fs::remove_file(&p).is_ok() {
                    ok = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            if !ok {
                left.push(p.file_name().unwrap_or_default().to_string_lossy().to_string());
            }
        }
    }
    if fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = fs::remove_dir(dir);
    }
    left
}

/// The uninstaller runs from the folder it removes, and a running program cannot delete itself. So it
/// copies itself to %TEMP%, starts that copy (directly, no shell) and exits; the copy waits for it to
/// end, removes the files, and is left in %TEMP% (the next Setup run clears old copies).
pub fn spawn_remover(dir: &Path, files: &[String]) -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    let copy = std::env::temp_dir().join(format!("GlintSetup-remove-{}.exe", std::process::id()));
    if fs::copy(&me, &copy).is_err() {
        log("      could not copy the uninstaller to %TEMP%");
        return false;
    }
    let r = Command::new(&copy)
        .arg("--remove-files")
        .arg(dir)
        .arg("--wait")
        .arg(std::process::id().to_string())
        .arg("--files")
        .arg(files.join("|"))
        .current_dir(std::env::temp_dir())
        .creation_flags(NO_WINDOW)
        .spawn();
    log(&format!("      file removal handed to {}: {}", copy.display(), if r.is_ok() { "started" } else { "failed" }));
    r.is_ok()
}

/// Copies left by earlier uninstalls (the running one cannot be deleted, which is fine).
fn clear_old_removers() {
    let me = std::env::current_exe().unwrap_or_default();
    if let Ok(rd) = fs::read_dir(std::env::temp_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("GlintSetup-remove-") && n.ends_with(".exe") && !same_path(&e.path(), &me) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

// ---------------------------------------------------------------- --yes (no window)

struct LogSink;

impl Sink for LogSink {
    fn step(&self, text: &str, index: usize, total: usize) {
        if total > 0 {
            log(&format!("      step {index} of {total}: {text}"));
        }
    }
    fn bytes(&self, _: u64, _: u64, _: f64) {}
    fn busy(&self, text: &str) {
        log(&format!("      {text}"));
    }
    fn can_cancel(&self, _: bool) {}
    fn cancelled(&self) -> bool {
        false
    }
}

pub fn temp_work_dir() -> PathBuf {
    std::env::temp_dir().join(format!("glint-setup-{}", std::process::id()))
}

fn quiet_install(o: &Opts) -> i32 {
    let tmp = temp_work_dir();
    let result = install(o, &tmp, &LogSink);
    let _ = fs::remove_dir_all(&tmp);
    match result {
        Ok(out) => {
            for n in &out.notes {
                log(&format!("Note: {n}"));
            }
            if o.launch {
                launch(&out.exe);
            }
            0
        }
        Err(e) => {
            log(&format!("Setup stopped: {e}"));
            1
        }
    }
}

fn quiet_uninstall(o: &Opts) -> i32 {
    let r = uninstall(o);
    if r.files_later && !spawn_remover(&o.dir, &r.files) {
        return 1;
    }
    if r.problems.is_empty() { 0 } else { 1 }
}

/// `--cancel-test <folder>`: start a real install into `<folder>\Glint` (test places, Glint only) and
/// press Cancel once 1 MB has arrived. Passes if it stops with "cancelled" and leaves nothing behind.
fn cancel_test(folder: &Path, mut o: Opts) -> i32 {
    struct CancelAt(std::sync::atomic::AtomicBool);
    impl Sink for CancelAt {
        fn step(&self, _: &str, _: usize, _: usize) {}
        fn bytes(&self, done: u64, _: u64, _: f64) {
            if done > 1_000_000 {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        fn busy(&self, _: &str) {}
        fn can_cancel(&self, _: bool) {}
        fn cancelled(&self) -> bool {
            self.0.load(std::sync::atomic::Ordering::Acquire)
        }
    }
    o.dir = folder.join("Glint");
    o.places = Places::test(&folder.join("places"));
    (o.ffmpeg, o.blur, o.start_menu, o.desktop, o.register, o.launch) = (false, false, false, false, false, false);
    let tmp = folder.join("work");
    let sink = CancelAt(std::sync::atomic::AtomicBool::new(false));
    let t = Instant::now();
    let r = install(&o, &tmp, &sink);
    let leftovers: Vec<String> = fs::read_dir(&tmp).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default();
    let _ = fs::remove_dir_all(&tmp);
    let ok = r.as_ref().err().map(String::as_str) == Some(CANCELLED) && !o.dir.exists() && !leftovers.iter().any(|n| n.ends_with(".zip"));
    log(&format!("cancel-test: {} after {:.1} s ({:?}; program folder made: {}; left in the work folder: {:?})", if ok { "PASS" } else { "FAIL" }, t.elapsed().as_secs_f64(), r.err(), o.dir.exists(), leftovers));
    if ok { 0 } else { 1 }
}

// ---------------------------------------------------------------- main

pub fn parse(args: &[String]) -> Opts {
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let places = match value("--test-root") {
        Some(root) => Places::test(&root),
        None => Places::real(),
    };
    let shortcuts = !has("--no-shortcuts");
    Opts {
        // When run as the uninstaller from inside the install folder, that folder is the default.
        dir: value("--dir").unwrap_or_else(|| match std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            Some(d) if has("--uninstall") && d.join("Glint.exe").is_file() => d,
            _ => env_dir("LOCALAPPDATA").join("Programs").join("Glint"),
        }),
        yes: has("--yes"),
        ffmpeg: !has("--no-ffmpeg"),
        force_ffmpeg: has("--force-ffmpeg"),
        blur: !has("--no-blur"),
        start_menu: shortcuts,
        desktop: false,
        register: shortcuts,
        launch: !has("--no-launch"),
        purge: false,
        places,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log_open();
    log(&format!("Glint setup ({}), {}", env!("CARGO_PKG_VERSION"), args.join(" ")));
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();

    if let Some(dir) = value("--remove-files") {
        if let Some(pid) = value("--wait").and_then(|p| p.parse::<u32>().ok()) {
            wait_pid(pid, 30_000);
        }
        let files: Vec<String> = value("--files").map(|f| f.split('|').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_else(|| KNOWN_FILES.iter().map(|s| s.to_string()).collect());
        let left = remove_files(Path::new(&dir), &files);
        log(&if left.is_empty() { format!("Removed the files in {dir}.") } else { format!("Left in {dir}: {}", left.join(", ")) });
        std::process::exit(if left.is_empty() { 0 } else { 1 });
    }
    let o = parse(&args);
    if let Some(folder) = value("--cancel-test") {
        std::process::exit(cancel_test(Path::new(&folder), o));
    }
    if let Some(folder) = value("--ui-test") {
        std::process::exit(setup_ui::ui_test(Path::new(&folder), o));
    }
    clear_old_removers();
    let code = match (args.iter().any(|a| a == "--uninstall"), o.yes) {
        (true, true) => quiet_uninstall(&o),
        (true, false) => setup_ui::run_remove(o),
        (false, true) => quiet_install(&o),
        (false, false) => setup_ui::run_wizard(o),
    };
    std::process::exit(code);
}
