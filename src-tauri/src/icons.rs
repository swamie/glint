//! Game icons by NAME, from what is installed on this PC, offline and without the game
//! running: Steam, Epic, GOG, Ubisoft, Riot, Xbox, Roblox, Minecraft launchers, the
//! uninstall entries in the registry and Start menu / desktop shortcuts.
//!
//! Three steps, so the matching can be tested on made-up data:
//! 1. `gather(host, wanted)` reads the registry and the disk through a `Host` and lists
//!    every installed thing it can name (`Sources`). Install folders are only searched
//!    for executables when their name matches one of the `wanted` games.
//! 2. `find(name, &sources)` / `find_all` is a pure function: the icon files that belong
//!    to that game, best first. A name must match exactly after `games::name_key`
//!    (case, punctuation, "™", "Tom Clancy's", edition words), or through `STORE_NAMES`:
//!    a wrong icon is worse than the coloured letter.
//! 3. `save` turns a source into a PNG (or copied image) in the icons folder.
use crate::core::Core;
use crate::games::{self, Games};
use crate::util;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Where one icon comes from: an .exe / .dll / .ico (with the icon's number in it) or an image file.
#[derive(Clone, Debug, PartialEq)]
pub struct IconSource {
    pub path: String,
    pub index: i32,
    /// Which store or list found it ("steam", "uninstall", ...), for the report.
    pub from: &'static str,
}

/// An executable found in an install folder.
#[derive(Clone, Debug)]
pub struct ExeFile {
    pub path: String,
    pub size: u64,
}

/// One installed thing with a name.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub name: String,
    pub from: &'static str,
    /// Icon files named by the store itself, best first.
    pub icons: Vec<(String, i32)>,
    /// The program the store starts (an Epic or GOG game's executable, a shortcut's target).
    pub exe: Option<String>,
    /// Install folder, if known.
    pub dir: Option<String>,
    /// Executables in `dir` (filled only for entries that match a wanted game).
    pub exes: Vec<ExeFile>,
    /// A small image that stands in when nothing better is there (Steam's 32 px library icon, an Xbox logo).
    pub fallback: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub entries: Vec<Entry>,
}

/// Registry roots the lookup reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hive {
    Lm,
    Cu,
}

pub struct DirItem {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
}

/// The registry and the disk as the lookup sees them. The real one reads Windows; the
/// self-test hands in made-up keys, environment and folders. Nothing is ever written.
pub trait Host {
    fn reg_keys(&self, hive: Hive, path: &str) -> Vec<String>;
    fn reg_value(&self, hive: Hive, path: &str, name: &str) -> Option<String>;
    fn env(&self, key: &str) -> String;
    /// Drive roots ("C:\\") that may hold an `XboxGames` folder.
    fn drives(&self) -> Vec<PathBuf>;
    fn list(&self, dir: &Path) -> Vec<DirItem> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| {
                let md = e.metadata().ok();
                DirItem { path: e.path(), is_dir: md.as_ref().is_some_and(|m| m.is_dir()), size: md.map(|m| m.len()).unwrap_or(0) }
            })
            .collect()
    }
    fn read(&self, file: &Path) -> Option<Vec<u8>> {
        fs::read(file).ok()
    }
    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }
}

/// This PC.
pub struct RealHost;

impl Host for RealHost {
    fn reg_keys(&self, hive: Hive, path: &str) -> Vec<String> {
        open(hive, path).map(|k| k.enum_keys().flatten().collect()).unwrap_or_default()
    }
    fn reg_value(&self, hive: Hive, path: &str, name: &str) -> Option<String> {
        let k = open(hive, path)?;
        k.get_value::<String, _>(name).ok().or_else(|| k.get_value::<u32, _>(name).ok().map(|v| v.to_string()))
    }
    fn env(&self, key: &str) -> String {
        std::env::var(key).unwrap_or_default()
    }
    fn drives(&self) -> Vec<PathBuf> {
        (b'C'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).filter(|p| p.join("XboxGames").is_dir()).collect()
    }
}

fn open(hive: Hive, path: &str) -> Option<winreg::RegKey> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    winreg::RegKey::predef(if hive == Hive::Lm { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER }).open_subkey(path).ok()
}

// ---------------------------------------------------------------------------
// Matching (pure)

/// Store names that differ from the name a clip carries: (game key, store key).
const STORE_NAMES: &[(&str, &str)] = &[
    ("roblox", "roblox player"),
    ("minecraft", "minecraft launcher"),
    ("overwatch", "overwatch 2"),
    ("rainbow six siege", "rainbow six siege x"),
    ("grand theft auto v", "grand theft auto v legacy"),
    ("grand theft auto v", "grand theft auto v enhanced"),
    ("call of duty black ops 6", "call of duty"),
    ("call of duty black ops 7", "call of duty"),
    ("call of duty warzone", "call of duty"),
    ("call of duty", "call of duty hq"),
    ("league of legends", "league of legends live"),
    ("counter strike 2", "counter strike global offensive"),
    ("pubg", "pubg battlegrounds"),
    ("osu", "osu lazer"),
];

/// Does a store's `entry` name stand for the game whose key is `game_key`?
fn matches(game_key: &str, entry_name: &str) -> bool {
    let ek = games::name_key(entry_name);
    !ek.is_empty() && (ek == game_key || STORE_NAMES.iter().any(|(g, s)| *g == game_key && *s == ek))
}

/// File names of programs whose icon is never the game's: uninstallers, set-ups, launchers of a whole store.
fn generic_icon(path: &str) -> bool {
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["unins", "uninst", "setup", "install", "msiexec", "update"].iter().any(|k| stem.starts_with(k))
        || [
            "upc", "uplay", "ubisoftconnect", "eadesktop", "ealauncher", "ealaunchhelper", "origin", "riotclientservices", "riotclientux", "battle.net",
            "battle.net launcher", "steam", "epicgameslauncher", "galaxyclient", "nvi2", "msedge", "explorer", "shell32", "imageres",
        ]
        .contains(&stem.as_str())
}

/// Executables in a game's folder that are not the game: crash reporters, anti-cheat, redistributables.
fn helper_exe(stem: &str) -> bool {
    ["crash", "unins", "redist", "setup", "report", "helper", "updater", "install", "anticheat", "battleye", "prereq", "dxweb", "cefprocess", "errorhandler", "uploader", "benchmark", "server"]
        .iter()
        .any(|k| stem.contains(k))
        || stem.ends_with("_be")
        || stem.ends_with("_eac")
        || ["eac", "be", "dxsetup", "vc_redist.x64", "vc_redist.x86", "dotnetfx", "7za", "steamerrorreporter", "notification_helper", "unitycrashhandler64", "unitycrashhandler32", "zfgamebrowser"]
            .contains(&stem)
}

fn usable(path: &str) -> bool {
    let ext = Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["exe", "dll", "ico", "png", "jpg", "jpeg", "bmp", "gif", "webp"].contains(&ext.as_str()) && !generic_icon(path)
}

/// Every icon on this PC that belongs to the game `name`, best first.
pub fn find_all(name: &str, src: &Sources) -> Vec<IconSource> {
    let key = games::name_key(name);
    if key.is_empty() || games::canonical(name) == games::DESKTOP || name == "Imported" {
        return vec![];
    }
    let mut scored: Vec<(u32, usize, IconSource)> = vec![];
    let mut push = |score: u32, path: &str, index: i32, from: &'static str| {
        if usable(path) {
            let n = scored.len();
            scored.push((score, n, IconSource { path: path.to_string(), index, from }));
        }
    };
    let compact = key.replace(' ', "");
    for e in src.entries.iter().filter(|e| matches(&key, &e.name)) {
        // An executable named after the game (or known to be it) in its own folder.
        let mut named = false;
        for x in &e.exes {
            let stem = Path::new(&x.path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            let by_table = games::known_exe_name(&stem).is_some_and(|n| matches(&key, n));
            let dir_key = e.dir.as_deref().and_then(|d| Path::new(d).file_name()).map(|f| games::name_key(&f.to_string_lossy()).replace(' ', "")).unwrap_or_default();
            let s = games::name_key(&stem).replace(' ', "");
            if by_table || s == compact || (!dir_key.is_empty() && s == dir_key) {
                push(100, &x.path, 0, e.from);
                named = true;
            }
        }
        if let Some(exe) = &e.exe {
            push(90, exe, 0, e.from);
        }
        for (p, i) in &e.icons {
            push(80, p, *i, e.from);
        }
        for p in &e.fallback {
            push(60, p, 0, e.from);
        }
        // Else the biggest program in its folder that is not a helper: it is still this game's own folder.
        if !named {
            if let Some(x) = e
                .exes
                .iter()
                .filter(|x| !helper_exe(&Path::new(&x.path).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()))
                .max_by_key(|x| x.size)
            {
                push(50, &x.path, 0, e.from);
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<IconSource> = vec![];
    for (_, _, s) in scored {
        if !out.iter().any(|o| o.path.eq_ignore_ascii_case(&s.path) && o.index == s.index) {
            out.push(s);
        }
    }
    out
}

/// The best icon on this PC for the game `name`, if any.
pub fn find(name: &str, src: &Sources) -> Option<IconSource> {
    find_all(name, src).into_iter().next()
}

// ---------------------------------------------------------------------------
// Reading what is installed

/// The quoted strings of a Steam .vdf / .acf text, in order (braces dropped, `\\` unescaped).
fn vdf_tokens(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        s.push(n);
                    }
                }
                '"' => break,
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

/// The values that follow `key` in a .vdf text.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let tok = vdf_tokens(text);
    tok.windows(2).filter(|w| w[0].eq_ignore_ascii_case(key)).map(|w| w[1].clone()).collect()
}

/// `"C:\Games\x.exe",-101` / `C:\Games\x.exe,0` / `%ProgramFiles%\x.ico` -> (path, index).
pub fn parse_icon_spec(spec: &str, host: &dyn Host) -> Option<(String, i32)> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    let (path, index) = match s.rfind(',') {
        Some(at) if s[at + 1..].trim().parse::<i32>().is_ok() => (&s[..at], s[at + 1..].trim().parse::<i32>().unwrap_or(0)),
        _ => (s, 0),
    };
    let path = expand_env(path.trim().trim_matches('"').trim(), host);
    (!path.is_empty()).then_some((path, index))
}

fn expand_env(s: &str, host: &dyn Host) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        let Some(b) = rest[a + 1..].find('%') else { break };
        let var = &rest[a + 1..a + 1 + b];
        let val = host.env(var);
        out.push_str(&rest[..a]);
        if val.is_empty() {
            out.push('%');
            out.push_str(var);
            out.push('%');
        } else {
            out.push_str(&val);
        }
        rest = &rest[a + b + 2..];
    }
    out.push_str(rest);
    out.replace('/', "\\")
}

/// A Windows shortcut (.lnk): (target program, icon location). Reads the parts of
/// MS-SHLLINK that carry them: LinkInfo's local path and the string data's icon location.
pub fn parse_lnk(b: &[u8]) -> Option<(Option<String>, Option<(String, i32)>)> {
    let u16_at = |o: usize| b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]]) as usize);
    let u32_at = |o: usize| b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize);
    if u32_at(0)? != 0x4C {
        return None;
    }
    let flags = u32_at(0x14)?;
    let icon_index = u32_at(0x38)? as u32 as i32;
    let mut at = 0x4C;
    if flags & 0x1 != 0 {
        at += 2 + u16_at(at)?; // LinkTargetIDList
    }
    let mut target = None;
    if flags & 0x2 != 0 {
        let size = u32_at(at)?;
        let header = u32_at(at + 4)?;
        let info_flags = u32_at(at + 8)?;
        if info_flags & 0x1 != 0 {
            let cstr = |o: usize| -> Option<String> {
                let s = b.get(o..at + size)?;
                let end = s.iter().position(|&c| c == 0)?;
                Some(String::from_utf8_lossy(&s[..end]).into_owned())
            };
            let wstr = |o: usize| -> Option<String> {
                let s = b.get(o..at + size)?;
                let w: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                Some(String::from_utf16_lossy(&w))
            };
            let wide = if header >= 0x24 { u32_at(at + 28).filter(|&o| o > 0).and_then(|o| wstr(at + o)) } else { None };
            target = wide.filter(|w| !w.is_empty()).or_else(|| u32_at(at + 16).and_then(|o| cstr(at + o)));
        }
        at += size;
    }
    let unicode = flags & 0x80 != 0;
    let mut icon = None;
    // String data, in this order, each present when its flag is set.
    for bit in [0x4, 0x8, 0x10, 0x20, 0x40] {
        if flags & bit == 0 {
            continue;
        }
        let n = u16_at(at)?;
        let bytes = if unicode { n * 2 } else { n };
        let raw = b.get(at + 2..at + 2 + bytes)?;
        let s = if unicode { String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()) } else { String::from_utf8_lossy(raw).into_owned() };
        if bit == 0x40 && !s.trim().is_empty() {
            icon = Some((s, icon_index));
        }
        at += 2 + bytes;
    }
    Some((target.filter(|t| !t.is_empty()), icon))
}

/// An Internet shortcut (.url, what Steam and Epic put on the desktop): its icon file and number.
pub fn parse_url(text: &str) -> Option<(String, i32)> {
    let mut file = None;
    let mut index = 0;
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("IconFile=") {
            file = Some(v.trim().to_string());
        } else if let Some(v) = l.strip_prefix("IconIndex=") {
            index = v.trim().parse().unwrap_or(0);
        }
    }
    file.filter(|f| !f.is_empty()).map(|f| (f, index))
}

/// The value of `attr="..."` in an XML tag text.
fn xml_attr(text: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=\"");
    let mut from = 0;
    while let Some(at) = text[from..].find(&pat) {
        let start = from + at;
        // A whole attribute name, not the tail of a longer one.
        let ok = start == 0 || !text[..start].ends_with(|c: char| c.is_alphanumeric() || c == ':');
        let vstart = start + pat.len();
        if ok {
            let end = text[vstart..].find('"')?;
            return Some(text[vstart..vstart + end].to_string());
        }
        from = vstart;
    }
    None
}

fn file_stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Executables in a game's folder, at most four folders deep, skipping the folders that
/// hold only redistributables, anti-cheat and engine tools.
fn list_exes(host: &dyn Host, dir: &Path) -> Vec<ExeFile> {
    const SKIP: &[&str] = &[
        "_commonredist", "commonredist", "redist", "redists", "redistributables", "directx", "vcredist", "dotnet", "support", "easyanticheat", "battleye",
        "__installer", "installer", "installers", "prereqs", "prerequisites", "thirdparty", "engine", "mono", "jre", "java", "python", "tools", "sdk", "crashpad",
    ];
    let mut out = vec![];
    let mut stack = vec![(dir.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        for it in host.list(&d) {
            seen += 1;
            if seen > 3000 {
                return out;
            }
            let name = it.path.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
            if it.is_dir {
                if depth < 4 && !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push((it.path, depth + 1));
                }
            } else if name.ends_with(".exe") {
                out.push(ExeFile { path: it.path.to_string_lossy().into_owned(), size: it.size });
            }
        }
    }
    out
}

const UNINSTALL: &[(Hive, &str)] = &[
    (Hive::Lm, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    (Hive::Lm, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    (Hive::Cu, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
];

/// Everything installed on this PC that has a name, read through `host`. `wanted`: the game
/// names to look up; only their install folders are searched and only their shortcuts read.
pub fn gather(host: &dyn Host, wanted: &[String]) -> Sources {
    let keys: Vec<String> = wanted.iter().map(|w| games::name_key(w)).filter(|k| !k.is_empty()).collect();
    let wanted_name = |n: &str| keys.iter().any(|k| matches(k, n));
    let mut entries: Vec<Entry> = vec![];

    // Steam: library folders -> app manifests -> install folder; the 32 px library icon as a fallback.
    let steam = host
        .reg_value(Hive::Cu, r"Software\Valve\Steam", "SteamPath")
        .or_else(|| host.reg_value(Hive::Lm, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
        .map(|p| p.replace('/', "\\"))
        .unwrap_or_else(|| format!(r"{}\Steam", host.env("ProgramFiles(x86)")));
    let steam = PathBuf::from(steam);
    let mut libs = vec![steam.clone()];
    if let Some(text) = host.read(&steam.join(r"steamapps\libraryfolders.vdf")) {
        for p in vdf_values(&String::from_utf8_lossy(&text), "path") {
            let p = PathBuf::from(p);
            if !libs.iter().any(|l| l.to_string_lossy().eq_ignore_ascii_case(&p.to_string_lossy())) {
                libs.push(p);
            }
        }
    }
    for lib in &libs {
        for it in host.list(&lib.join("steamapps")) {
            let file = it.path.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Some(text) = host.read(&it.path) else { continue };
            let text = String::from_utf8_lossy(&text);
            let (Some(name), Some(dir)) = (vdf_values(&text, "name").into_iter().next(), vdf_values(&text, "installdir").into_iter().next()) else { continue };
            let appid = vdf_values(&text, "appid").into_iter().next().unwrap_or_default();
            let mut fallback = vec![];
            if !appid.is_empty() {
                // `appcache\librarycache\<appid>\<40 hex>.jpg` is the game's small icon.
                for f in host.list(&steam.join(r"appcache\librarycache").join(&appid)) {
                    let stem = file_stem(&f.path);
                    let ext = f.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if !f.is_dir && ext == "jpg" && stem.len() == 40 && stem.chars().all(|c| c.is_ascii_hexdigit()) {
                        fallback.push(f.path.to_string_lossy().into_owned());
                    }
                }
            }
            let dir = lib.join(r"steamapps\common").join(&dir).to_string_lossy().into_owned();
            entries.push(Entry { name, from: "steam", dir: Some(dir), fallback, ..Default::default() });
        }
    }

    // Epic: one JSON manifest per installed game.
    let epic = PathBuf::from(host.env("ProgramData")).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    for it in host.list(&epic) {
        if !it.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("item")) {
            continue;
        }
        let Some(v) = host.read(&it.path).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (name, dir, launch) = (s("DisplayName"), s("InstallLocation"), s("LaunchExecutable"));
        if name.is_empty() {
            continue;
        }
        let exe = (!dir.is_empty() && !launch.is_empty()).then(|| Path::new(&dir).join(launch.replace('/', "\\")).to_string_lossy().into_owned());
        entries.push(Entry { name, from: "epic", exe, dir: (!dir.is_empty()).then_some(dir), ..Default::default() });
    }

    // Uninstall entries: DisplayName + DisplayIcon (+ InstallLocation).
    for (hive, root) in UNINSTALL {
        for k in host.reg_keys(*hive, root) {
            let path = format!(r"{root}\{k}");
            let Some(name) = host.reg_value(*hive, &path, "DisplayName").filter(|n| !n.trim().is_empty()) else { continue };
            let icons = host.reg_value(*hive, &path, "DisplayIcon").and_then(|s| parse_icon_spec(&s, host)).into_iter().collect();
            let dir = host.reg_value(*hive, &path, "InstallLocation").map(|d| expand_env(d.trim().trim_matches('"'), host)).filter(|d| !d.is_empty());
            entries.push(Entry { name: name.trim().to_string(), from: "uninstall", icons, dir, ..Default::default() });
        }
    }

    // GOG Galaxy.
    for root in [r"SOFTWARE\WOW6432Node\GOG.com\Games", r"SOFTWARE\GOG.com\Games"] {
        for k in host.reg_keys(Hive::Lm, root) {
            let path = format!(r"{root}\{k}");
            let Some(name) = host.reg_value(Hive::Lm, &path, "gameName") else { continue };
            let exe = host.reg_value(Hive::Lm, &path, "exe").filter(|e| !e.is_empty());
            let dir = host.reg_value(Hive::Lm, &path, "path").filter(|e| !e.is_empty());
            entries.push(Entry { name, from: "gog", exe, dir, ..Default::default() });
        }
    }

    // Ubisoft Connect: install folders, named after the folder.
    for root in [r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs", r"SOFTWARE\Ubisoft\Launcher\Installs"] {
        for k in host.reg_keys(Hive::Lm, root) {
            let Some(dir) = host.reg_value(Hive::Lm, &format!(r"{root}\{k}"), "InstallDir") else { continue };
            let dir = dir.replace('/', "\\").trim_end_matches('\\').to_string();
            let name = Path::new(&dir).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if !name.is_empty() {
                entries.push(Entry { name, from: "ubisoft", dir: Some(dir), ..Default::default() });
            }
        }
    }

    // Riot: `ProgramData\Riot Games\Metadata\<game>.live\<game>.live.ico`.
    for it in host.list(&PathBuf::from(host.env("ProgramData")).join(r"Riot Games\Metadata")) {
        if !it.is_dir {
            continue;
        }
        let id = it.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let name = id.trim_end_matches(".live").replace('_', " ");
        let icons: Vec<(String, i32)> = host.list(&it.path).into_iter().filter(|f| f.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ico"))).map(|f| (f.path.to_string_lossy().into_owned(), 0)).collect();
        if !icons.is_empty() {
            entries.push(Entry { name, from: "riot", icons, ..Default::default() });
        }
    }

    // Xbox / PC Game Pass: `<drive>\XboxGames\<game>\Content\MicrosoftGame.config`.
    for drive in host.drives() {
        for it in host.list(&drive.join("XboxGames")) {
            if !it.is_dir {
                continue;
            }
            let content = it.path.join("Content");
            let Some(cfg) = host.read(&content.join("MicrosoftGame.config")) else { continue };
            let cfg = String::from_utf8_lossy(&cfg);
            let name = xml_attr(&cfg, "DefaultDisplayName").filter(|n| !n.starts_with("ms-resource")).unwrap_or_else(|| file_stem(&it.path));
            let exe = cfg.find("<Executable").and_then(|at| xml_attr(&cfg[at..], "Name")).map(|e| content.join(e.replace('/', "\\")).to_string_lossy().into_owned());
            let fallback = ["Square150x150Logo", "Square44x44Logo", "StoreLogo"].iter().filter_map(|a| xml_attr(&cfg, a)).map(|l| content.join(l.replace('/', "\\")).to_string_lossy().into_owned()).collect();
            entries.push(Entry { name, from: "xbox", exe, dir: Some(content.to_string_lossy().into_owned()), fallback, ..Default::default() });
        }
    }

    // Roblox and Minecraft: their programs sit in known places.
    let local = host.env("LOCALAPPDATA");
    for base in [format!(r"{local}\Roblox\Versions"), format!(r"{}\Roblox\Versions", host.env("ProgramFiles(x86)"))] {
        for v in host.list(Path::new(&base)).into_iter().filter(|v| v.is_dir) {
            let exe = v.path.join("RobloxPlayerBeta.exe");
            if host.is_file(&exe) {
                entries.push(Entry { name: "Roblox".into(), from: "roblox", exe: Some(exe.to_string_lossy().into_owned()), ..Default::default() });
            }
        }
    }
    let (pf86, pf) = (host.env("ProgramFiles(x86)"), host.env("ProgramFiles"));
    let xbox_mc = host.drives().into_iter().map(|d| d.join(r"XboxGames\Minecraft Launcher\Content\Minecraft.exe").to_string_lossy().into_owned());
    for p in [format!(r"{pf86}\Minecraft Launcher\MinecraftLauncher.exe"), format!(r"{pf}\Minecraft Launcher\MinecraftLauncher.exe")].into_iter().chain(xbox_mc).chain([
        format!(r"{local}\Programs\lunarclient\Lunar Client.exe"),
        format!(r"{local}\Programs\Badlion Client\Badlion Client.exe"),
        format!(r"{local}\Programs\feather\Feather Launcher.exe"),
        format!(r"{local}\Programs\PrismLauncher\prismlauncher.exe"),
    ]) {
        if host.is_file(Path::new(&p)) {
            entries.push(Entry { name: "Minecraft".into(), from: "minecraft", icons: vec![(p, 0)], ..Default::default() });
        }
    }

    // Start menu and desktop shortcuts, named by their file name. Only the wanted ones are read.
    let (appdata, pdata, profile, public) = (host.env("APPDATA"), host.env("ProgramData"), host.env("USERPROFILE"), host.env("PUBLIC"));
    let mut stack: Vec<(PathBuf, u32)> = [
        format!(r"{appdata}\Microsoft\Windows\Start Menu\Programs"),
        format!(r"{pdata}\Microsoft\Windows\Start Menu\Programs"),
        format!(r"{profile}\Desktop"),
        format!(r"{public}\Desktop"),
    ]
    .into_iter()
    .filter(|p| !p.starts_with('\\'))
    .map(|p| (PathBuf::from(p), 0))
    .collect();
    let mut seen = 0;
    while let Some((d, depth)) = stack.pop() {
        for it in host.list(&d) {
            seen += 1;
            if seen > 5000 {
                break;
            }
            if it.is_dir {
                if depth < 3 {
                    stack.push((it.path, depth + 1));
                }
                continue;
            }
            let ext = it.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let name = file_stem(&it.path);
            if !(ext == "lnk" || ext == "url") || !wanted_name(&name) {
                continue;
            }
            let Some(bytes) = host.read(&it.path) else { continue };
            if ext == "url" {
                if let Some((f, i)) = parse_url(&String::from_utf8_lossy(&bytes)) {
                    entries.push(Entry { name, from: "shortcut", icons: vec![(expand_env(&f, host), i)], ..Default::default() });
                }
            } else if let Some((target, icon)) = parse_lnk(&bytes) {
                let icons = icon.map(|(f, i)| (expand_env(&f, host), i)).into_iter().collect();
                let exe = target.filter(|t| t.to_lowercase().ends_with(".exe")).map(|t| expand_env(&t, host));
                entries.push(Entry { name, from: "shortcut", icons, exe, ..Default::default() });
            }
        }
    }

    // Keep only what exists, and search the folders of the wanted games for executables.
    for e in entries.iter_mut() {
        e.icons.retain(|(p, _)| host.is_file(Path::new(p)));
        e.fallback.retain(|p| host.is_file(Path::new(p)));
        if e.exe.as_deref().is_some_and(|x| !host.is_file(Path::new(x))) {
            e.exe = None;
        }
        if let Some(d) = e.dir.clone() {
            if wanted_name(&e.name) {
                e.exes = list_exes(host, Path::new(&d));
            }
        }
    }
    Sources { entries }
}

// ---------------------------------------------------------------------------
// Saving and running

/// Put the icon in the icons folder: an .exe / .dll / .ico as a 64 px PNG, an image as a copy.
/// Each file gets a new name, so the window never shows an old copy from its cache.
pub fn save(name: &str, s: &IconSource) -> Option<String> {
    const SIZE: u32 = 64;
    let ext = Path::new(&s.path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let base = format!("{}-{}", util::sanitize_filename(name), util::new_id());
    if ["exe", "dll", "ico"].contains(&ext.as_str()) {
        let rgba = crate::win::exe_icon_rgba_at(&s.path, s.index, SIZE)?;
        if rgba.chunks_exact(4).all(|p| p[3] == 0) {
            return None;
        }
        let path = games::icons_dir().join(format!("{base}.png"));
        let file = fs::File::create(&path).ok()?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let ok = enc.write_header().ok().and_then(|mut w| w.write_image_data(&rgba).ok()).is_some();
        if !ok {
            let _ = fs::remove_file(&path);
            return None;
        }
        Some(path.to_string_lossy().into_owned())
    } else {
        // Only small images: a game's icon, not a 4 MB cover.
        let len = fs::metadata(&s.path).map(|m| m.len()).unwrap_or(0);
        if len == 0 || len > 2_000_000 {
            return None;
        }
        let path = games::icons_dir().join(format!("{base}.{ext}"));
        fs::copy(&s.path, &path).ok()?;
        Some(path.to_string_lossy().into_owned())
    }
}

/// The first of `cands` that turns into an icon file.
fn save_first(name: &str, cands: &[IconSource]) -> Option<(String, IconSource)> {
    cands.iter().find_map(|c| save(name, c).map(|p| (p, c.clone())))
}

/// Look up an icon for every game that has none (and no picked one). Returns how many got one.
pub fn fill_missing(games: &Mutex<Games>, host: &dyn Host) -> usize {
    let todo = games.lock().unwrap().missing_icons();
    if todo.is_empty() {
        return 0;
    }
    let names: Vec<String> = todo.iter().map(|g| g.name.clone()).collect();
    let src = gather(host, &names);
    let mut got = 0;
    for g in &todo {
        let mut cands = vec![];
        if !g.exe.is_empty() && Path::new(&g.exe).is_file() && g.name != "Minecraft" {
            cands.push(IconSource { path: g.exe.clone(), index: 0, from: "seen running" });
        }
        cands.extend(find_all(&g.name, &src));
        if let Some((path, from)) = save_first(&g.name, &cands) {
            if games.lock().unwrap().set_found_icon(&g.name, &path, false) {
                util::log(&format!("icons: {} <- {} ({})", g.name, from.path, from.from));
                got += 1;
            } else {
                let _ = fs::remove_file(&path); // the user picked one meanwhile
            }
        }
    }
    got
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static AGAIN: AtomicBool = AtomicBool::new(false);

/// `fill_missing` on a background thread (start-up, after an import or a rescan); emits
/// "games-changed" when icons arrived. A call while one runs makes it run once more after.
pub fn fill_in_background(core: Arc<Core>) {
    AGAIN.store(true, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            AGAIN.store(false, Ordering::SeqCst);
            if fill_missing(&core.games, &RealHost) > 0 {
                core.emit("games-changed", ());
            }
            if !AGAIN.load(Ordering::SeqCst) {
                break;
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
        // A call that came in between the last check and the store above.
        if AGAIN.load(Ordering::SeqCst) {
            fill_in_background(core);
        }
    });
}

/// "Find icon again": look the game up now and replace its icon (a picked one too). None: nothing found.
pub fn find_again(games: &Mutex<Games>, host: &dyn Host, name: &str) -> Option<games::Game> {
    let exe = games.lock().unwrap().list.iter().find(|g| g.name.eq_ignore_ascii_case(name)).map(|g| g.exe.clone()).unwrap_or_default();
    let src = gather(host, &[name.to_string()]);
    let mut cands = vec![];
    if !exe.is_empty() && Path::new(&exe).is_file() && name != "Minecraft" {
        cands.push(IconSource { path: exe, index: 0, from: "seen running" });
    }
    cands.extend(find_all(name, &src));
    let (path, _) = save_first(name, &cands)?;
    let mut g = games.lock().unwrap();
    if !g.list.iter().any(|x| x.name.eq_ignore_ascii_case(name)) {
        g.list.push(games::Game { name: name.to_string(), ..Default::default() });
    }
    g.set_found_icon(name, &path, true);
    g.list.iter().find(|x| x.name.eq_ignore_ascii_case(name)).cloned()
}

// ---------------------------------------------------------------------------
// Self-test: glint.exe --icons-test <report.txt>

/// Made-up registry and environment over real made-up folders.
struct FakeHost {
    reg: BTreeMap<(Hive, String), BTreeMap<String, String>>,
    env: BTreeMap<String, String>,
    drives: Vec<PathBuf>,
}

impl FakeHost {
    fn set(&mut self, hive: Hive, path: &str, name: &str, value: &str) {
        self.reg.entry((hive, path.to_lowercase())).or_default().insert(name.to_lowercase(), value.to_string());
    }
}

impl Host for FakeHost {
    fn reg_keys(&self, hive: Hive, path: &str) -> Vec<String> {
        let pre = format!("{}\\", path.to_lowercase());
        let mut out: Vec<String> = self.reg.keys().filter(|(h, p)| *h == hive && p.starts_with(&pre)).map(|(_, p)| p[pre.len()..].split('\\').next().unwrap_or("").to_string()).collect();
        out.dedup();
        out
    }
    fn reg_value(&self, hive: Hive, path: &str, name: &str) -> Option<String> {
        self.reg.get(&(hive, path.to_lowercase()))?.get(&name.to_lowercase()).cloned()
    }
    fn env(&self, key: &str) -> String {
        self.env.get(key).cloned().unwrap_or_default()
    }
    fn drives(&self) -> Vec<PathBuf> {
        self.drives.clone()
    }
}

struct Report {
    lines: Vec<String>,
    fails: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let d = detail.as_ref();
        if !ok {
            self.fails += 1;
        }
        self.lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if ok || d.is_empty() { String::new() } else { format!(" -- {d}") }));
    }
}

/// A shortcut file with an icon location and (optionally) a local target path.
fn make_lnk(target: Option<&str>, icon: Option<(&str, i32)>) -> Vec<u8> {
    let mut flags = 0x80u32; // IsUnicode
    if target.is_some() {
        flags |= 0x2;
    }
    if icon.is_some() {
        flags |= 0x40;
    }
    let mut b = vec![0u8; 0x4C];
    b[0..4].copy_from_slice(&0x4Cu32.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&flags.to_le_bytes());
    b[0x38..0x3C].copy_from_slice(&icon.map(|i| i.1).unwrap_or(0).to_le_bytes());
    if let Some(t) = target {
        // LinkInfo with a 0x1C header and an ANSI local base path.
        let mut path = t.as_bytes().to_vec();
        path.push(0);
        let size = 0x1C + path.len() + 1;
        let mut li = vec![0u8; 0x1C];
        li[0..4].copy_from_slice(&(size as u32).to_le_bytes());
        li[4..8].copy_from_slice(&0x1Cu32.to_le_bytes());
        li[8..12].copy_from_slice(&1u32.to_le_bytes());
        li[16..20].copy_from_slice(&0x1Cu32.to_le_bytes());
        li.extend(path);
        li.push(0);
        b.extend(li);
    }
    if let Some((p, _)) = icon {
        let w: Vec<u16> = p.encode_utf16().collect();
        b.extend((w.len() as u16).to_le_bytes());
        for c in w {
            b.extend(c.to_le_bytes());
        }
    }
    b
}

pub fn self_test(report_path: &str) {
    let mut r = Report { lines: vec![], fails: 0 };
    if std::env::var("GLINT_DATA_DIR").map(|d| d.is_empty()).unwrap_or(true) {
        let _ = fs::write(report_path, "FAIL set GLINT_DATA_DIR to a scratch folder first\n");
        return;
    }

    // Names: keys, same_game, and canonical unchanged.
    let k = games::name_key;
    r.check("key: case, punctuation, trademark", k("Tom Clancy's Rainbow Six® Siege") == "rainbow six siege" && k("Tom Clancys Rainbow Six Siege") == "rainbow six siege", k("Tom Clancy's Rainbow Six® Siege"));
    r.check("key: Garry's Mod", k("Garry's Mod") == k("Garrys Mod") && k("Garry’s Mod") == "garrys mod", k("Garry’s Mod"));
    r.check("key: editions", k("Detroit: Become Human™ Deluxe Edition") == "detroit become human" && k("Raft Game of the Year Edition") == "raft" && k("Subnautica GOTY") == "subnautica", k("Raft Game of the Year Edition"));
    r.check("key: Legacy is part of a name", k("Hogwarts Legacy") == "hogwarts legacy", "");
    r.check("key: skate. and Schedule I", k("skate.") == "skate" && k("Schedule I") == "schedule i" && k("STAR WARS™ Battlefront™ II") == "star wars battlefront ii", "");
    r.check("same_game: Medal's folder spelling", games::same_game("Tom Clancys Rainbow Six Siege", "Rainbow Six Siege"), "");
    r.check("same_game: Overwatch 2 / Overwatch", games::same_game("Overwatch 2", "Overwatch"), "");
    r.check("same_game: different games stay apart", !games::same_game("BACKROOMS", "Escape the Backrooms") && !games::same_game("EA Sports FC 25", "EA Sports FC 26") && !games::same_game("Geometry Dash", "Geometry Dash 2"), "");
    r.check("same_game: empty is no game", !games::same_game("", "") && !games::same_game("™", "®"), "");
    r.check(
        "canonical unchanged",
        games::canonical("Overwatch 2") == "Overwatch" && games::canonical("tom clancy's rainbow six siege") == "Rainbow Six Siege" && games::canonical("Lunar Client (1.8.9)") == "Minecraft" && games::canonical("Screen Recording") == games::DESKTOP && games::canonical("Raft") == "Raft",
        "",
    );

    // Parsers.
    let dummy = FakeHost { reg: BTreeMap::new(), env: [("ProgramFiles".to_string(), r"C:\PF".to_string())].into(), drives: vec![] };
    r.check("icon spec: quoted with index", parse_icon_spec("\"C:\\A B\\x.exe\",-101", &dummy) == Some((r"C:\A B\x.exe".into(), -101)), "");
    r.check("icon spec: env and no index", parse_icon_spec("%ProgramFiles%\\G\\g.ico", &dummy) == Some((r"C:\PF\G\g.ico".into(), 0)), format!("{:?}", parse_icon_spec("%ProgramFiles%\\G\\g.ico", &dummy)));
    r.check("icon spec: comma in a folder name", parse_icon_spec(r"C:\Games, Inc\g.exe", &dummy) == Some((r"C:\Games, Inc\g.exe".into(), 0)), "");
    r.check("vdf: values unescaped", vdf_values("\"0\"\n{\n\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n}\n\"1\" { \"path\" \"E:\\\\S\" }", "path") == vec![r"D:\SteamLibrary".to_string(), r"E:\S".to_string()], "");
    r.check("url shortcut", parse_url("[InternetShortcut]\r\nURL=steam://rungameid/1\r\nIconIndex=2\r\nIconFile=C:\\S\\g.ico\r\n") == Some((r"C:\S\g.ico".into(), 2)), "");
    let lnk = parse_lnk(&make_lnk(Some(r"C:\G\raft.exe"), Some((r"C:\G\raft.ico", 3))));
    r.check("lnk: target and icon", lnk == Some((Some(r"C:\G\raft.exe".into()), Some((r"C:\G\raft.ico".into(), 3)))), format!("{lnk:?}"));
    r.check("lnk: garbage is none", parse_lnk(b"not a shortcut").is_none() && parse_lnk(&[0x4C, 0, 0, 0]).is_none(), "");

    // A made-up PC.
    let root = util::tmp_dir().join(format!("icons-test-{}", util::new_id()));
    let _ = fs::remove_dir_all(&root);
    let me = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mk = |p: &Path, bytes: &[u8]| {
        let _ = fs::create_dir_all(p.parent().unwrap());
        let _ = fs::write(p, bytes);
    };
    let exe_bytes = fs::read(&me).unwrap_or_default(); // Glint's own exe: it carries an icon
    let put_exe = |p: &Path| mk(p, &exe_bytes);
    let s = |p: &Path| p.to_string_lossy().into_owned();
    let steam = root.join("Steam");
    let lib2 = root.join("Lib2");
    mk(&steam.join(r"steamapps\libraryfolders.vdf"), format!("\"libraryfolders\"\n{{\n\"0\" {{ \"path\" \"{}\" }}\n\"1\" {{ \"path\" \"{}\" }}\n}}\n", s(&steam).replace('\\', "\\\\"), s(&lib2).replace('\\', "\\\\")).as_bytes());
    mk(&steam.join(r"steamapps\appmanifest_4000.acf"), b"\"AppState\"\n{\n\"appid\" \"4000\"\n\"name\" \"Garry's Mod\"\n\"installdir\" \"GarrysMod\"\n}\n");
    put_exe(&steam.join(r"steamapps\common\GarrysMod\hl2.exe"));
    put_exe(&steam.join(r"steamapps\common\GarrysMod\bin\win64\gmod.exe"));
    mk(&steam.join(r"appcache\librarycache\4000\0123456789abcdef0123456789abcdef01234567.jpg"), b"\xff\xd8\xff\xe0 fake jpg");
    mk(&lib2.join(r"steamapps\appmanifest_730.acf"), b"\"AppState\" { \"appid\" \"730\" \"name\" \"Counter-Strike 2\" \"installdir\" \"Counter-Strike Global Offensive\" }");
    put_exe(&lib2.join(r"steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe"));
    mk(&lib2.join(r"steamapps\common\Counter-Strike Global Offensive\game\bin\win64\crashhandler64.exe"), &vec![0u8; exe_bytes.len() * 2]);
    mk(&lib2.join(r"steamapps\appmanifest_9.acf"), b"\"AppState\" { \"appid\" \"9\" \"name\" \"Subnautica\" \"installdir\" \"Subnautica\" }");
    put_exe(&lib2.join(r"steamapps\common\Subnautica\Subnautica.exe"));
    put_exe(&lib2.join(r"steamapps\common\Subnautica\UnityCrashHandler64.exe"));
    mk(&root.join(r"PD\Epic\EpicGamesLauncher\Data\Manifests\A1.item"), serde_json::json!({ "DisplayName": "Fortnite", "InstallLocation": s(&root.join(r"Epic\Fortnite")), "LaunchExecutable": "FortniteGame/Binaries/Win64/FortniteLauncher.exe" }).to_string().as_bytes());
    put_exe(&root.join(r"Epic\Fortnite\FortniteGame\Binaries\Win64\FortniteLauncher.exe"));
    mk(&root.join(r"PD\Epic\EpicGamesLauncher\Data\Manifests\A2.item"), b"{ broken json");
    put_exe(&root.join(r"Ubi\Tom Clancy's Rainbow Six Siege\RainbowSix.exe"));
    put_exe(&root.join(r"Ubi\Tom Clancy's Rainbow Six Siege\RainbowSix_BE.exe"));
    mk(&root.join(r"PD\Riot Games\Metadata\valorant.live\valorant.live.ico"), b"ico");
    put_exe(&root.join(r"Local\Roblox\Versions\version-abc\RobloxPlayerBeta.exe"));
    put_exe(&root.join(r"Local\Programs\lunarclient\Lunar Client.exe"));
    put_exe(&root.join(r"Games\Rocket\RocketLeague.exe"));
    put_exe(&root.join(r"Games\Rocket\unins000.exe"));
    put_exe(&root.join(r"Games\GB\Gang Beasts.exe"));
    put_exe(&root.join(r"Games\Java\java.exe"));
    put_exe(&root.join(r"Games\Escape\EscapeTheBackrooms.exe"));
    put_exe(&root.join(r"Games\Geo2\GeometryDash2.exe"));
    put_exe(&root.join(r"Games\Raft\Raft.exe"));
    put_exe(&root.join(r"Games\Shortcut\s.exe"));
    mk(&root.join(r"Games\Raft\raft.ico"), b"ico");
    mk(&root.join(r"Games\Sub\sub.ico"), b"ico");
    put_exe(&root.join(r"Games\Detroit\DetroitBecomeHuman.exe"));
    mk(&root.join(r"Drive\XboxGames\Forza Horizon 5\Content\MicrosoftGame.config"), b"<Game><Executable Name=\"ForzaHorizon5.exe\" Id=\"Game\"/><ShellVisuals DefaultDisplayName=\"Forza Horizon 5\" Square150x150Logo=\"Logo150.png\" Square44x44Logo=\"Logo44.png\"/></Game>");
    mk(&root.join(r"Drive\XboxGames\Forza Horizon 5\Content\Logo44.png"), b"\x89PNG fake");
    // Start menu: a Steam-style .url, a .lnk with an icon, a .lnk to a program, an unwanted one.
    let sm = root.join(r"AD\Microsoft\Windows\Start Menu\Programs");
    mk(&sm.join(r"Steam\Subnautica.url"), format!("[InternetShortcut]\r\nURL=steam://rungameid/9\r\nIconFile={}\r\nIconIndex=0\r\n", s(&root.join(r"Games\Sub\sub.ico"))).as_bytes());
    mk(&sm.join("Raft.lnk"), &make_lnk(None, Some((&s(&root.join(r"Games\Raft\raft.ico")), 0))));
    mk(&sm.join(r"Games\Bloons TD 6.lnk"), &make_lnk(Some(&s(&root.join(r"Games\Shortcut\s.exe"))), None));
    mk(&sm.join("Some Tool.lnk"), b"never read");

    let mut h = FakeHost { reg: BTreeMap::new(), env: BTreeMap::new(), drives: vec![root.join("Drive")] };
    for (k, v) in [("ProgramData", "PD"), ("LOCALAPPDATA", "Local"), ("APPDATA", "AD"), ("USERPROFILE", "User"), ("PUBLIC", "Public"), ("ProgramFiles(x86)", "PF86"), ("ProgramFiles", "PF")] {
        h.env.insert(k.into(), s(&root.join(v)));
    }
    h.set(Hive::Cu, r"Software\Valve\Steam", "SteamPath", &s(&steam).replace('\\', "/"));
    let un = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    let un_cu = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let add = |h: &mut FakeHost, hive: Hive, base: &str, key: &str, vals: &[(&str, String)]| {
        for (n, v) in vals {
            h.set(hive, &format!(r"{base}\{key}"), n, v);
        }
    };
    add(&mut h, Hive::Lm, un, "Rocket", &[("DisplayName", "Rocket League®".into()), ("DisplayIcon", format!("\"{}\",0", s(&root.join(r"Games\Rocket\unins000.exe")))), ("InstallLocation", s(&root.join(r"Games\Rocket")))]);
    add(&mut h, Hive::Lm, un, "GangBeasts", &[("DisplayName", "Gang Beasts".into()), ("DisplayIcon", s(&root.join(r"Games\GB\Gang Beasts.exe")))]);
    add(&mut h, Hive::Lm, un, "Java", &[("DisplayName", "Java".into()), ("DisplayIcon", s(&root.join(r"Games\Java\java.exe")))]);
    add(&mut h, Hive::Lm, un, "Escape", &[("DisplayName", "Escape the Backrooms".into()), ("DisplayIcon", s(&root.join(r"Games\Escape\EscapeTheBackrooms.exe")))]);
    add(&mut h, Hive::Lm, un, "Geo2", &[("DisplayName", "Geometry Dash 2".into()), ("DisplayIcon", s(&root.join(r"Games\Geo2\GeometryDash2.exe")))]);
    add(&mut h, Hive::Lm, un, "Missing", &[("DisplayName", "Cyberpunk 2077".into()), ("DisplayIcon", s(&root.join(r"Games\Gone\Cyberpunk2077.exe")))]);
    add(&mut h, Hive::Cu, un_cu, "Detroit", &[("DisplayName", "Detroit: Become Human™ Deluxe Edition".into()), ("DisplayIcon", format!("{},0", s(&root.join(r"Games\Detroit\DetroitBecomeHuman.exe"))))]);
    add(&mut h, Hive::Lm, r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs", "635", &[("InstallDir", s(&root.join(r"Ubi\Tom Clancy's Rainbow Six Siege")).replace('\\', "/") + "/")]);

    let wanted: Vec<String> = [
        "Garry's Mod", "Counter-Strike 2", "Subnautica", "Fortnite", "Tom Clancys Rainbow Six Siege", "Valorant", "Roblox", "Minecraft", "Rocket League", "Gang Beasts", "BACKROOMS",
        "Escape the Backrooms", "Geometry Dash", "Cyberpunk 2077", "Detroit Become Human", "Forza Horizon 5", "Raft", "Bloons TD 6", "Some Tool", "Apex Legends",
    ]
    .iter()
    .map(|x| x.to_string())
    .collect();
    let src = gather(&h, &wanted);
    let f = |n: &str| find(n, &src);
    let ends = |n: &str, tail: &str| f(n).is_some_and(|x| x.path.to_lowercase().ends_with(&tail.to_lowercase()));
    r.check("steam: two library folders read", src.entries.iter().filter(|e| e.from == "steam").count() == 3, format!("{}", src.entries.iter().filter(|e| e.from == "steam").count()));
    r.check("steam: exe named in the table wins over a bigger helper", ends("Counter-Strike 2", r"win64\cs2.exe"), format!("{:?}", f("Counter-Strike 2")));
    r.check("steam: exe named after the game wins over the crash handler", ends("Subnautica", r"Subnautica\Subnautica.exe"), format!("{:?}", f("Subnautica")));
    r.check("steam: .url shortcut icon is a candidate too", find_all("Subnautica", &src).iter().any(|x| x.path.ends_with("sub.ico")), "");
    r.check("steam: library icon comes before a guessed exe", find_all("Garry's Mod", &src).first().is_some_and(|x| x.path.ends_with(".jpg")), format!("{:?}", find_all("Garry's Mod", &src)));
    r.check("steam: a guessed exe is still offered", find_all("Garrys Mod", &src).iter().any(|x| x.path.to_lowercase().ends_with(".exe")), "");
    r.check("epic: launch executable", ends("Fortnite", "FortniteLauncher.exe"), format!("{:?}", f("Fortnite")));
    r.check("ubisoft: folder name with Tom Clancy's, exe from the table", ends("Tom Clancys Rainbow Six Siege", r"\RainbowSix.exe") && ends("Rainbow Six Siege", r"\RainbowSix.exe"), format!("{:?}", f("Rainbow Six Siege")));
    r.check("riot: metadata icon", ends("Valorant", "valorant.live.ico"), format!("{:?}", f("Valorant")));
    r.check("roblox: versions folder", ends("Roblox", "RobloxPlayerBeta.exe"), "");
    r.check("minecraft: a launcher's icon, never Java's", ends("Minecraft", "Lunar Client.exe") && !find_all("Minecraft", &src).iter().any(|x| x.path.ends_with("java.exe")), format!("{:?}", find_all("Minecraft", &src)));
    r.check("uninstall: ® ignored, uninstaller icon refused, install folder used", ends("Rocket League", "RocketLeague.exe") && !find_all("Rocket League", &src).iter().any(|x| x.path.contains("unins")), format!("{:?}", find_all("Rocket League", &src)));
    r.check("uninstall: plain DisplayIcon", ends("Gang Beasts", "Gang Beasts.exe"), "");
    r.check("uninstall: edition words and ™", ends("Detroit Become Human", "DetroitBecomeHuman.exe"), format!("{:?}", f("Detroit Become Human")));
    r.check("never wrong: BACKROOMS is not Escape the Backrooms", f("BACKROOMS").is_none() && ends("Escape the Backrooms", "EscapeTheBackrooms.exe"), format!("{:?}", f("BACKROOMS")));
    r.check("never wrong: Geometry Dash is not Geometry Dash 2", f("Geometry Dash").is_none(), "");
    r.check("missing icon file is dropped", f("Cyberpunk 2077").is_none(), format!("{:?}", f("Cyberpunk 2077")));
    r.check("xbox: MicrosoftGame.config, logo when the exe is not there", ends("Forza Horizon 5", "Logo44.png"), format!("{:?}", find_all("Forza Horizon 5", &src)));
    r.check("start menu: .lnk icon location", ends("Raft", "raft.ico"), "");
    r.check("start menu: .lnk target program", ends("Bloons TD 6", r"Shortcut\s.exe"), "");
    r.check("not installed keeps the letter", f("Apex Legends").is_none() && f("Desktop").is_none() && f("Imported").is_none() && f("").is_none(), "");
    r.check("unwanted shortcuts are not read", !src.entries.iter().any(|e| e.name == "Some Tool") || f("Some Tool").is_none(), "");

    // Saving and the background fill on a made-up games list.
    let saved = save("Test Game", &IconSource { path: me.clone(), index: 0, from: "test" });
    r.check("save: exe icon becomes a 64 px PNG in the icons folder", saved.as_deref().is_some_and(|p| p.ends_with(".png") && Path::new(p).is_file() && Path::new(p).starts_with(games::icons_dir())), format!("{saved:?}"));
    let png_ok = saved.as_deref().and_then(|p| fs::read(p).ok()).is_some_and(|b| b.starts_with(b"\x89PNG"));
    r.check("save: PNG signature", png_ok, "");
    r.check("save: a fake .ico gives nothing (never an empty icon)", save("X", &IconSource { path: s(&root.join(r"Games\Raft\raft.ico")), index: 0, from: "test" }).is_none(), "");
    let copied = save("Forza", &IconSource { path: s(&root.join(r"Drive\XboxGames\Forza Horizon 5\Content\Logo44.png")), index: 0, from: "test" });
    r.check("save: an image is copied", copied.as_deref().is_some_and(|p| p.ends_with(".png") && fs::read(p).ok().as_deref() == Some(b"\x89PNG fake")), format!("{copied:?}"));
    if let Some(p) = &saved {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = &copied {
        let _ = fs::remove_file(p);
    }

    let custom = games::icons_dir().join("picked-test.png");
    let _ = fs::write(&custom, b"picked");
    let list = vec![
        games::Game { name: "Counter-Strike 2".into(), ..Default::default() },
        games::Game { name: "Gang Beasts".into(), icon: s(&custom), custom_icon: true, ..Default::default() },
        games::Game { name: "Fortnite".into(), icon: s(&root.join("gone.png")), ..Default::default() },
        games::Game { name: "Apex Legends".into(), ..Default::default() },
        games::Game { name: games::DESKTOP.into(), ..Default::default() },
    ];
    let gm = Mutex::new(Games { list });
    let n = fill_missing(&gm, &h);
    let snap = gm.lock().unwrap().list.clone();
    let icon_of = |n: &str| snap.iter().find(|g| g.name == n).map(|g| g.icon.clone()).unwrap_or_default();
    r.check("fill: two games got an icon", n == 2, format!("{n}"));
    r.check("fill: icon file written", Path::new(&icon_of("Counter-Strike 2")).is_file() && Path::new(&icon_of("Fortnite")).is_file(), "");
    r.check("fill: a picked icon is never replaced", icon_of("Gang Beasts") == s(&custom) && snap.iter().any(|g| g.name == "Gang Beasts" && g.custom_icon), "");
    r.check("fill: not installed keeps the letter", icon_of("Apex Legends").is_empty() && icon_of(games::DESKTOP).is_empty(), "");
    r.check("fill: second run finds nothing to do", fill_missing(&gm, &h) == 0, "");
    let before = icon_of("Counter-Strike 2");
    let again = find_again(&gm, &h, "Gang Beasts");
    r.check("find again: replaces a picked icon when asked", again.as_ref().is_some_and(|g| !g.custom_icon && g.icon != s(&custom) && Path::new(&g.icon).is_file()) && !custom.exists(), format!("{again:?}"));
    let again2 = find_again(&gm, &h, "Counter-Strike 2");
    r.check("find again: new file, old one removed", again2.as_ref().is_some_and(|g| g.icon != before) && !Path::new(&before).exists(), "");
    r.check("find again: nothing found leaves it alone", find_again(&gm, &h, "Apex Legends").is_none(), "");
    for g in gm.lock().unwrap().list.iter() {
        if Path::new(&g.icon).starts_with(games::icons_dir()) {
            let _ = fs::remove_file(&g.icon);
        }
    }
    let _ = fs::remove_dir_all(&root);

    // This PC, read only: how many of these names resolve (no file written outside the scratch data folder).
    let mut names: Vec<String> = vec![];
    if let Ok(text) = fs::read_to_string(PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join(r"Glint\games.json")) {
        if let Ok(v) = serde_json::from_str::<Vec<games::Game>>(&text) {
            names.extend(v.into_iter().map(|g| g.name));
        }
    }
    let friend = [
        "Roblox", "Fortnite", "Rocket League", "Tom Clancys Rainbow Six Siege", "Garry's Mod", "Counter-Strike 2", "BACKROOMS", "Minecraft", "Cyberpunk 2077", "Grand Theft Auto V",
        "EA Sports FC 26", "Subnautica", "Escape the Backrooms", "Rematch", "Schedule I", "skate.", "Call of Duty: Black Ops 6", "EA Sports FC 25", "Gang Beasts", "Geometry Dash",
        "Apex Legends", "How to Fish", "Bloons TD 6", "Detroit Become Human", "Forza Horizon 5", "Raft", "Star Wars Battlefront II",
    ];
    names.extend(friend.iter().map(|x| x.to_string()));
    names.retain(|n| n != games::DESKTOP);
    let mut uniq: Vec<String> = vec![];
    for n in names {
        if !uniq.iter().any(|u| u.eq_ignore_ascii_case(&n)) {
            uniq.push(n);
        }
    }
    let started = std::time::Instant::now();
    let real = gather(&RealHost, &uniq);
    let ms = started.elapsed().as_millis();
    let mut hits = 0;
    let mut extracted = 0;
    for n in &uniq {
        let all = find_all(n, &real);
        let made = save_first(n, &all);
        if !all.is_empty() {
            hits += 1;
        }
        if let Some((p, _)) = &made {
            extracted += 1;
            let _ = fs::remove_file(p);
        }
        r.lines.push(format!("INFO this PC: {n} -> {}", all.first().map(|x| format!("{} [{}]{}", x.path, x.from, if made.is_some() { "" } else { " (could not be read)" })).unwrap_or_else(|| "none".into())));
    }
    r.lines.push(format!("INFO this PC: {hits} of {} names matched, {extracted} icons extracted; {} entries gathered in {ms} ms", uniq.len(), real.entries.len()));

    let total = r.lines.iter().filter(|l| l.starts_with("PASS") || l.starts_with("FAIL")).count();
    r.lines.push(format!("{} of {total} checks pass", total - r.fails));
    let _ = fs::write(report_path, r.lines.join("\n") + "\n");
}
