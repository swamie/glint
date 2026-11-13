use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static T0: OnceLock<Instant> = OnceLock::new();

/// Monotonic seconds since process start. Every timeline in the app uses this clock.
pub fn now_secs() -> f64 {
    T0.get_or_init(Instant::now).elapsed().as_secs_f64()
}

pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn new_id() -> String {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    format!("{:x}{:02x}", unix_ms(), SEQ.fetch_add(1, Ordering::Relaxed) & 0xff)
}

/// App data folder. `GLINT_DATA_DIR` overrides it (used by the benchmark).
pub fn data_dir() -> PathBuf {
    let dir = match std::env::var_os("GLINT_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Glint"),
    };
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Append a line to `<data dir>/glint.log` (trimmed once it passes 1 MB).
pub fn log(msg: &str) {
    use std::io::Write;
    let path = data_dir().join("glint.log");
    if fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = fs::remove_file(&path);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), msg);
    }
}

pub fn tmp_dir() -> PathBuf {
    let d = data_dir().join("tmp");
    let _ = fs::create_dir_all(&d);
    d
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;

/// A child process that never flashes a console window.
pub fn hidden_cmd(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

pub fn hidden_cmd_prio(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY_CLASS);
    c
}

/// Locate a tool: explicit path, next to our exe, `%LOCALAPPDATA%\Glint\bin`, then PATH. Scoop shims are
/// resolved to the real exe so kill() and CPU stats hit the right process.
pub fn find_exe(custom: &str, name: &str) -> Option<PathBuf> {
    let file = format!("{name}.exe");
    let mut cands: Vec<PathBuf> = Vec::new();
    if !custom.trim().is_empty() {
        cands.push(PathBuf::from(custom.trim()));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join(&file));
            cands.push(d.join("bin").join(&file));
        }
    }
    // Where "Check and repair" puts ffmpeg when Glint's own folder is read-only (deps.rs).
    cands.push(crate::deps::local_bin_dir().join(&file));
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            cands.push(d.join(&file));
        }
    }
    cands.into_iter().find(|c| c.is_file()).map(resolve_shim)
}

fn resolve_shim(p: PathBuf) -> PathBuf {
    if let Ok(txt) = fs::read_to_string(p.with_extension("shim")) {
        for line in txt.lines() {
            if let Some(rest) = line.trim().strip_prefix("path") {
                let v = rest.trim_start().trim_start_matches('=').trim().trim_matches('"');
                let target = PathBuf::from(v);
                if target.is_file() {
                    return target;
                }
            }
        }
    }
    p
}

pub fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() { "Clip".into() } else { cleaned.chars().take(60).collect() }
}

/// Name of the library folder for a game's clips ("Other" if the game is unknown).
pub fn game_folder(game: &str) -> String {
    if game.trim().trim_matches('.').trim().is_empty() { "Other".into() } else { sanitize_filename(game) }
}

/// The Explorer argument that opens a file's folder with the file selected.
///
/// Explorer does not follow the usual quoting rules: only the path may be in
/// quotes, never the whole `/select,…` argument. Quoted as a whole (which is
/// what happens to any argument with a space in it) Explorer cannot read it
/// and opens Documents instead. Explorer also wants backslashes.
pub fn select_arg(path: &Path) -> String {
    format!("/select,\"{}\"", path.to_string_lossy().replace('/', "\\"))
}

/// Open the folder that holds `path` in Explorer, with the file selected. If
/// the file is gone, open the nearest folder that still exists.
pub fn reveal(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("explorer.exe");
    if path.exists() {
        cmd.raw_arg(select_arg(path));
    } else {
        let mut dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        while !dir.as_os_str().is_empty() && !dir.is_dir() {
            if !dir.pop() {
                break;
            }
        }
        if !dir.is_dir() {
            return Err("The clip's folder no longer exists".into());
        }
        cmd.raw_arg(format!("\"{}\"", dir.to_string_lossy().replace('/', "\\")));
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// First free path for `base.ext` inside `dir` (adds " (2)", " (3)", ...).
pub fn unique_path(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

pub fn last_lines(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}
