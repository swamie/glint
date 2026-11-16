//! GAME mode's detection, rebuilt in 0.1.41: what is a game, which window to record, and how.
//!
//! One pure function, `decide`, takes a snapshot of the top-level windows (`win::WinInfo`), the
//! monitors, the installed-game libraries read from disk (`Library`), the user's choices
//! (`Prefs`), a few facts about this PC (`Env`) and what was decided before (`Memory`), and
//! returns a `Decision`: the window, the source (the window, its monitor by Desktop Duplication,
//! its monitor by the configured method, paused, or waiting), WHY (`Why`, shown by the UI and
//! written to glint.log once per change), and every candidate with its evidence (the switch's menu).
//!
//! Layers of evidence, each with a reason code (`Ev::code`) and points:
//! (a) the user: the pick (by exe path AND exe name; anti-cheat suffixes folded), a launcher it
//!     hands over to (same install folder, or the store of a picked store launcher), the "game"
//!     marks; the "never" list beats everything;
//! (b) libraries installed on this PC, read from disk only: Steam, Epic, Riot, Xbox / Game Pass,
//!     GOG, Ubisoft, Blizzard and EA (their install keys); an exe inside an install folder is a game,
//!     named after the install;
//! (c) the known table, games played before, store folders, and the window: covering its monitor,
//!     a popup without a caption (a swap chain's window), in front for a while;
//! (d) `DENY`: things that are never games, in one table.
//!
//! Nothing here opens another process: window facts, the shell's notification state, monitor
//! facts and files of the launchers are all it reads.
use crate::games::{self, Game};
use crate::win::WinInfo;
use serde::Serialize;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// (d) Never games. One table: add a program's file name (lower case, no ".exe") to its group.

pub const DENY: &[(&str, &[&str])] = &[
    ("launcher", &[
        "steam", "steamwebhelper", "steamservice", "epicgameslauncher", "epicwebhelper", "battle.net", "battle.net launcher", "riotclientservices", "riotclientux",
        "riotclientuxrender", "leagueclient", "leagueclientux", "leagueclientuxrender", "eadesktop", "eabackgroundservice", "origin", "upc", "ubisoftconnect",
        "uplaywebcore", "galaxyclient", "galaxyclient helper", "playnite.desktopapp", "playnite.fullscreenapp", "xboxpcapp", "xboxapp", "gamingservices",
        "minecraftlauncher", "minecraft launcher", "lunar client", "badlion client", "feather launcher", "prismlauncher", "multimc", "curseforge", "modrinth app",
        "atlauncher", "gdlauncher", "tlauncher", "overwolf", "itch", "rockstarservice", "socialclubhelper", "launcher", "amazon games ui", "heroic",
    ]),
    ("browser", &["chrome", "msedge", "firefox", "opera", "opera_gx", "brave", "vivaldi", "arc", "zen", "iexplore", "msedgewebview2", "librewolf", "waterfox", "chromium", "thorium"]),
    ("chat", &["discord", "discordptb", "discordcanary", "vesktop", "slack", "teams", "ms-teams", "zoom", "telegram", "whatsapp", "signal", "skype", "guilded", "teamspeak", "ts3client_win64", "mumble"]),
    ("capture", &[
        "obs64", "obs32", "obs", "medal", "medalencoder", "glint", "glint-setup", "kite", "nvidia app", "nvidia share", "geforce experience", "streamlabs obs",
        "streamlabs desktop", "sharex", "lightshot", "bandicam", "fraps", "xsplit.core", "outplayed", "snippingtool", "screenclippinghost",
    ]),
    ("overlay", &[
        "gamebar", "gamebarftserver", "gamebarpresencewriter", "xboxgamebarwidgets", "nvcontainer", "nvidia overlay", "amdrsserv", "radeonsoftware", "rtss",
        "msiafterburner", "losslessscaling", "borderlessgaming", "wallpaper32", "wallpaper64", "wallpaperservice32", "wallpaperui", "lively", "rainmeter", "steamvr", "vrserver", "vrmonitor",
        "vrcompositor", "oculusclient",
    ]),
    ("player", &["vlc", "mpv", "mpc-hc64", "mpc-hc", "mpc-be64", "wmplayer", "video.ui", "microsoft.media.player", "potplayermini64", "potplayermini", "photos", "spotify", "spotifast", "itunes", "foobar2000", "musicbee", "plex", "stremio"]),
    ("editor", &[
        "code", "devenv", "rider64", "idea64", "notepad", "notepad++", "sublime_text", "blender", "photoshop", "premiere pro", "afterfx", "resolve", "winword",
        "excel", "powerpnt", "outlook", "acrobat", "onenote", "obsidian", "notion", "figma", "krita", "audacity", "capcut", "vegas",
    ]),
    ("tool", &[
        "windowsterminal", "powershell", "pwsh", "cmd", "conhost", "claude", "taskmgr", "mmc", "regedit", "7zfm", "winrar", "qbittorrent", "everything",
        "expressvpn", "expressvpn.vpn.ui", "nordvpn", "protonvpn", "surfshark", "mullvad vpn", "wireguard", "openvpn-gui", "anydesk", "teamviewer",
        "rustdesk", "parsecd", "moonlight", "processhacker", "systeminformer", "hwinfo64", "cpuz", "gpu-z",
    ]),
    ("shell", &[
        "explorer", "searchhost", "searchapp", "shellexperiencehost", "startmenuexperiencehost", "applicationframehost", "lockapp", "textinputhost", "dwm",
        "widgets", "phoneexperiencehost", "systemsettings",
    ]),
];

/// The deny group of a program (`stem`: file name, lower case, no ".exe"), if it is never a game.
pub fn denied(stem: &str) -> Option<&'static str> {
    DENY.iter().find(|(_, list)| list.contains(&stem)).map(|(kind, _)| *kind)
}

/// Parts of a program's name that mark a helper beside a game, not the game itself.
const HELPER_WORDS: &[&str] = &["helper", "crash", "launcher", "updater", "service", "overlay", "installer", "setup", "redist", "anticheat", "easyanticheat", "battleye", "report", "bootstrap"];

/// Anti-cheat wrappers start the game as another file: "Game_BE.exe", "Game_EAC.exe".
const AC_SUFFIXES: &[&str] = &["_eac_eos", "_eac", "_be", "_eos", "-eac", "_battleye", "_protected"];

fn stem_of(path: &str) -> String {
    Path::new(&path.to_lowercase()).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A program's family: its stem without an anti-cheat suffix ("fortniteclient-win64-shipping_eac_eos" -> "fortniteclient-win64-shipping").
pub fn family(stem: &str) -> String {
    let s = stem.to_lowercase();
    let s = s.trim_end_matches(".exe");
    for suf in AC_SUFFIXES {
        if let Some(base) = s.strip_suffix(suf) {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    s.to_string()
}

fn norm_dir(p: &str) -> String {
    let mut d = p.trim().replace('/', "\\").to_lowercase();
    while d.ends_with('\\') {
        d.pop();
    }
    if d.is_empty() {
        return d;
    }
    d.push('\\');
    d
}

// ---------------------------------------------------------------------------
// (b) Installed libraries, read from disk. Each reader is tolerant of missing and odd files.

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Install {
    /// "steam" | "epic" | "riot" | "xbox" | "gog" | "ubisoft" | "blizzard" | "ea"
    pub store: &'static str,
    pub name: String,
    /// The install folder, lower case, with one backslash at the end.
    pub dir: String,
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub installs: Vec<Install>,
}

impl Library {
    /// The install an executable lies in (the deepest folder that holds it).
    pub fn find(&self, exe: &str) -> Option<&Install> {
        let lower = exe.replace('/', "\\").to_lowercase();
        self.installs.iter().filter(|i| i.dir.len() > 3 && lower.starts_with(&i.dir)).max_by_key(|i| i.dir.len())
    }

    fn push(&mut self, store: &'static str, name: &str, dir: &str) {
        let dir = norm_dir(dir);
        let name = name.trim();
        // A drive root or a store's own common folder is no install.
        if dir.len() <= 3 || name.is_empty() || dir.ends_with("\\steamapps\\common\\") || self.installs.iter().any(|i| i.dir == dir) {
            return;
        }
        self.installs.push(Install { store, name: name.to_string(), dir });
    }
}

/// The quoted strings and braces of a Valve KeyValues file (`libraryfolders.vdf`, `appmanifest_*.acf`).
fn vdf_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                s.push(match n { 'n' => '\n', 't' => '\t', other => other });
                            }
                        }
                        '"' => break,
                        other => s.push(other),
                    }
                }
                out.push(s);
            }
            '{' | '}' => out.push(c.to_string()),
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Every `"key" "value"` pair of a KeyValues text, at any depth (keys lower case).
pub fn vdf_pairs(text: &str) -> Vec<(String, String)> {
    let toks = vdf_tokens(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < toks.len() {
        let (k, v) = (&toks[i], &toks[i + 1]);
        if k != "{" && k != "}" && v != "{" && v != "}" {
            out.push((k.to_lowercase(), v.clone()));
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// The library folders a `libraryfolders.vdf` lists: `"path"` entries (new format) or numbered
/// entries holding a path (old format).
pub fn steam_library_paths(vdf: &str) -> Vec<String> {
    vdf_pairs(vdf)
        .into_iter()
        .filter(|(k, v)| k == "path" || (k.chars().all(|c| c.is_ascii_digit()) && v.contains(":\\")))
        .map(|(_, v)| v)
        .collect()
}

/// (name, installdir, appid) of an `appmanifest_*.acf`; None when it lacks them.
pub fn steam_manifest(acf: &str) -> Option<(String, String, String)> {
    let pairs = vdf_pairs(acf);
    let get = |k: &str| pairs.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
    let dir = get("installdir").filter(|d| !d.trim().is_empty())?;
    Some((get("name").unwrap_or_else(|| dir.clone()), dir, get("appid").unwrap_or_default()))
}

/// Steam apps that are no games (Steamworks redistributables, SteamVR, Wallpaper Engine, the Proton runtime,
/// Lossless Scaling and Borderless Gaming: tools sold on Steam, seen in the live check of 0.1.41).
const STEAM_NOT_GAMES: &[&str] = &["228980", "250820", "431960", "1070560", "1391110", "1628350", "1493710", "993090", "388080"];

/// Every Steam install under the Steam folder `root` and the libraries it lists.
pub fn steam_installs(root: &Path) -> Vec<Install> {
    let mut lib = Library::default();
    let mut folders: Vec<PathBuf> = vec![root.to_path_buf()];
    for vdf in [root.join("steamapps").join("libraryfolders.vdf"), root.join("config").join("libraryfolders.vdf")] {
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            for p in steam_library_paths(&text) {
                let p = PathBuf::from(p);
                if !folders.iter().any(|f| norm_dir(&f.to_string_lossy()) == norm_dir(&p.to_string_lossy())) {
                    folders.push(p);
                }
            }
        }
    }
    for f in folders {
        let apps = f.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            let Some((name, dir, appid)) = std::fs::read_to_string(e.path()).ok().as_deref().and_then(steam_manifest) else { continue };
            if STEAM_NOT_GAMES.contains(&appid.as_str()) {
                continue;
            }
            lib.push("steam", &name, &apps.join("common").join(&dir).to_string_lossy());
        }
    }
    lib.installs
}

/// Every Epic Games install from the launcher's manifests folder (`*.item`, JSON). Items whose
/// categories do not say "games" (the Unreal Engine, tools) are left out.
pub fn epic_installs(dir: &Path) -> Vec<Install> {
    let mut lib = Library::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return lib.installs };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().to_lowercase().ends_with(".item") {
            continue;
        }
        let Some(v) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        if let Some(cats) = v.get("AppCategories").and_then(|c| c.as_array()) {
            if !cats.iter().any(|c| c.as_str().is_some_and(|c| c.eq_ignore_ascii_case("games"))) {
                continue;
            }
        }
        if v.get("bIsIncompleteInstall").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        let loc = s("InstallLocation");
        let name = if s("DisplayName").is_empty() { Path::new(&loc).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() } else { s("DisplayName") };
        lib.push("epic", &name, &loc);
    }
    lib.installs
}

/// Riot's games from `RiotClientInstalls.json` (`associated_client`: install folder -> client).
pub fn riot_installs(json: &str) -> Vec<Install> {
    let mut lib = Library::default();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return lib.installs };
    if let Some(map) = v.get("associated_client").and_then(|m| m.as_object()) {
        for dir in map.keys() {
            let d = dir.replace('/', "\\");
            // "C:\Riot Games\VALORANT\live\" -> "VALORANT"
            let parts: Vec<&str> = d.split('\\').filter(|p| !p.is_empty()).collect();
            let name = parts.iter().position(|p| p.eq_ignore_ascii_case("riot games")).and_then(|i| parts.get(i + 1)).or(parts.iter().rev().find(|p| !p.eq_ignore_ascii_case("live"))).copied().unwrap_or_default();
            lib.push("riot", name, &d);
        }
    }
    lib.installs
}

/// The folder names a drive's `.GamingRoot` file names (the Xbox app's games folder): "RGBX",
/// four bytes, then zero-terminated UTF-16 paths relative to the drive.
pub fn gaming_root_folders(bytes: &[u8]) -> Vec<String> {
    if bytes.len() < 10 || &bytes[..4] != b"RGBX" {
        return Vec::new();
    }
    let units: Vec<u16> = bytes[8..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    units.split(|u| *u == 0).map(String::from_utf16_lossy).map(|s| s.trim().trim_start_matches('\\').to_string()).filter(|s| !s.is_empty() && !s.contains(':')).collect()
}

/// Xbox / Game Pass games on the drive `root` ("D:\"): each folder of its games folder is one.
pub fn xbox_installs(root: &Path) -> Vec<Install> {
    let mut lib = Library::default();
    let Ok(bytes) = std::fs::read(root.join(".GamingRoot")) else { return lib.installs };
    for folder in gaming_root_folders(&bytes) {
        let Ok(rd) = std::fs::read_dir(root.join(&folder)) else { continue };
        for e in rd.flatten().filter(|e| e.path().is_dir()) {
            let name = e.file_name().to_string_lossy().into_owned();
            // The Xbox app keeps its saves folder and launchers (Minecraft Launcher) there too: no games.
            let lower = name.to_lowercase();
            if lower == "gamesave" || lower.contains("launcher") {
                continue;
            }
            lib.push("xbox", &name, &e.path().to_string_lossy());
        }
    }
    lib.installs
}

/// Uninstall entries of a store's games: (publisher, display name, install location) -> installs
/// of `store` for entries of `publisher` that are not the launcher itself.
pub fn uninstall_installs(entries: &[(String, String, String)], publisher: &str, store: &'static str, skip: &[&str]) -> Vec<Install> {
    let mut lib = Library::default();
    for (p, name, loc) in entries {
        if p.trim().eq_ignore_ascii_case(publisher) && !loc.trim().is_empty() && !skip.iter().any(|s| name.trim().eq_ignore_ascii_case(s)) {
            lib.push(store, name, loc);
        }
    }
    lib.installs
}

/// The library index is rebuilt at most this often (seconds), or sooner when a program that is in
/// no install comes to the front (`library_hint`), but never more than once per `LIB_MIN_SECS`.
pub const LIB_MAX_AGE_SECS: u64 = 300;
pub const LIB_MIN_SECS: u64 = 30;

struct LibCache {
    lib: Library,
    at: Option<std::time::Instant>,
    busy: bool,
    asked: bool,
}

static LIB: std::sync::Mutex<LibCache> = std::sync::Mutex::new(LibCache { lib: Library { installs: Vec::new() }, at: None, busy: false, asked: false });

/// The libraries of this PC (files of the launchers on disk and their install keys), from the
/// cache: never waits on the disk. A stale or asked-for refresh runs on a background thread
/// (read-only: no file is opened for writing, no key written, no process started).
pub fn library() -> Library {
    use std::time::Duration;
    let mut c = LIB.lock().unwrap();
    let age = c.at.map(|t| t.elapsed());
    let stale = age.map_or(true, |a| a >= Duration::from_secs(LIB_MAX_AGE_SECS) || (c.asked && a >= Duration::from_secs(LIB_MIN_SECS)));
    if stale && !c.busy {
        c.busy = true;
        c.asked = false;
        std::thread::Builder::new()
            .name("game-library".into())
            .spawn(|| {
                let lib = scan();
                let mut c = LIB.lock().unwrap();
                c.lib = lib;
                c.at = Some(std::time::Instant::now());
                c.busy = false;
            })
            .ok();
    }
    c.lib.clone()
}

/// A program in front lies in no known install: the index may be out of date (a game installed
/// since); the next `library()` refreshes it in the background (at most once per `LIB_MIN_SECS`).
pub fn library_hint(front_exe: &str, lib: &Library) {
    let lower = front_exe.to_lowercase();
    if lower.is_empty() || lib.find(&lower).is_some() || denied(&stem_of(&lower)).is_some() || lower.contains("\\windows\\") {
        return;
    }
    LIB.lock().unwrap().asked = true;
}

/// Fixed local drives only (never network, removable or optical: those can stall or wake a disk).
fn fixed_drives() -> Vec<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    (b'C'..=b'Z')
        .filter_map(|d| {
            let root = format!("{}:\\", d as char);
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            (unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } == 3).then(|| PathBuf::from(root)) // DRIVE_FIXED
        })
        .collect()
}

/// Read every library now (blocking; library() runs it in the background).
pub fn scan() -> Library {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let mut lib = Library::default();
    let add = |v: Vec<Install>, lib: &mut Library| {
        for i in v {
            lib.push(i.store, &i.name, &i.dir);
        }
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let steam = hkcu.open_subkey(r"Software\Valve\Steam").and_then(|k| k.get_value::<String, _>("SteamPath")).map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(format!(r"{}\Steam", env("ProgramFiles(x86)"))));
    add(steam_installs(&steam), &mut lib);
    let pd = env("ProgramData");
    add(epic_installs(&Path::new(&pd).join(r"Epic\EpicGamesLauncher\Data\Manifests")), &mut lib);
    if let Ok(t) = std::fs::read_to_string(Path::new(&pd).join(r"Riot Games\RiotClientInstalls.json")) {
        add(riot_installs(&t), &mut lib);
    }
    for root in fixed_drives() {
        if root.join(".GamingRoot").is_file() {
            add(xbox_installs(&root), &mut lib);
        }
    }
    // GOG and Ubisoft keep one key per installed game.
    if let Ok(gog) = hklm.open_subkey(r"SOFTWARE\WOW6432Node\GOG.com\Games") {
        for k in gog.enum_keys().flatten().filter_map(|n| gog.open_subkey(n).ok()) {
            let (name, path) = (k.get_value::<String, _>("gameName").unwrap_or_default(), k.get_value::<String, _>("path").unwrap_or_default());
            lib.push("gog", &name, &path);
        }
    }
    if let Ok(ubi) = hklm.open_subkey(r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs") {
        for k in ubi.enum_keys().flatten().filter_map(|n| ubi.open_subkey(n).ok()) {
            let dir = k.get_value::<String, _>("InstallDir").unwrap_or_default();
            let name = Path::new(dir.trim_end_matches(['/', '\\'])).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            lib.push("ubisoft", &name, &dir);
        }
    }
    // Blizzard (Battle.net) and EA app games register ordinary uninstall entries.
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for path in [r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"] {
        if let Ok(un) = hklm.open_subkey(path) {
            for k in un.enum_keys().flatten().filter_map(|n| un.open_subkey(n).ok()) {
                let g = |v: &str| k.get_value::<String, _>(v).unwrap_or_default();
                let p = g("Publisher");
                if p.eq_ignore_ascii_case("Blizzard Entertainment") || p.eq_ignore_ascii_case("Electronic Arts") {
                    entries.push((p, g("DisplayName"), g("InstallLocation")));
                }
            }
        }
    }
    add(uninstall_installs(&entries, "Blizzard Entertainment", "blizzard", &["Battle.net"]), &mut lib);
    add(uninstall_installs(&entries, "Electronic Arts", "ea", &["EA app", "Origin"]), &mut lib);
    lib
}

// ---------------------------------------------------------------------------
// (a)-(c) Evidence for one window.

/// Folders that game stores install into (lower case, with surrounding backslashes).
const GAME_DIRS: &[&str] = &[
    "\\steamapps\\common\\", "\\epic games\\", "\\riot games\\", "\\xboxgames\\", "\\modifiablewindowsapps\\", "\\ubisoft game launcher\\games\\",
    "\\origin games\\", "\\ea games\\", "\\gog galaxy\\games\\", "\\gog games\\", "\\roblox\\versions\\", "\\games\\",
];

/// The user's choices for GAME mode (settings.rs).
pub struct Prefs<'a> {
    pub auto: bool,
    /// The program picked by hand: its file name (`games::exe_key`) and, from 0.1.41, its full path.
    pub pick: &'a str,
    pub pick_path: &'a str,
    pub games: &'a [String],
    /// "Never treat this as a game".
    pub not_games: &'a [String],
    pub seen: &'a [Game],
}

impl Prefs<'_> {
    pub fn none() -> Prefs<'static> {
        Prefs { auto: true, pick: "", pick_path: "", games: &[], not_games: &[], seen: &[] }
    }
}

/// One piece of evidence: a reason code the UI translates, and its points.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Ev {
    /// "pick" | "handoff" | "marked" | "known" | "minecraft" | "library:<store>" | "seen" | "folder"
    /// | "fullscreen" | "gamewindow" | "front"
    pub code: String,
    pub points: i64,
}

fn ev(code: &str, points: i64) -> Ev {
    Ev { code: code.into(), points }
}

/// A program counts as a game with at least this many points.
pub const GAME_MIN: i64 = 80;
/// Smaller windows are splash screens and pop-ups (a picked program is taken at any size).
const MIN_W: u32 = 320;
const MIN_H: u32 = 200;
/// "In front for a while".
pub const FRONT_SECS: f64 = 10.0;

/// The store whose games a picked store launcher hands over to.
fn launcher_store(stem: &str) -> Option<&'static str> {
    Some(match stem {
        "steam" | "steamwebhelper" => "steam",
        "epicgameslauncher" => "epic",
        "riotclientservices" | "riotclientux" | "riotclientuxrender" => "riot",
        "battle.net" | "battle.net launcher" => "blizzard",
        "eadesktop" | "origin" => "ea",
        "upc" | "ubisoftconnect" => "ubisoft",
        "galaxyclient" => "gog",
        "xboxpcapp" | "xboxapp" => "xbox",
        _ => return None,
    })
}

/// A launcher: one of the deny table's, or a program called or kept in a "launcher".
fn launcherish(exe: &str) -> bool {
    let stem = stem_of(exe);
    denied(&stem) == Some("launcher") || stem.contains("launcher") || exe.to_lowercase().contains("\\launcher\\")
}

/// The install folder of the picked program: its library install, else its own folder with
/// launcher / binary folders climbed out of (at most three).
pub fn pick_root(pick_path: &str, lib: &Library) -> Option<String> {
    if pick_path.trim().is_empty() {
        return None;
    }
    if let Some(i) = lib.find(pick_path) {
        return Some(i.dir.clone());
    }
    let mut dir = Path::new(&pick_path.to_lowercase().replace('/', "\\")).parent()?.to_path_buf();
    for _ in 0..3 {
        let name = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if ["launcher", "bin", "binaries", "win64", "win32", "x64", "x86", "live", "_retail_", "game", "client"].contains(&name.as_str()) {
            match dir.parent() {
                Some(p) if p.parent().is_some() => dir = p.to_path_buf(),
                _ => break,
            }
        } else {
            break;
        }
    }
    let d = norm_dir(&dir.to_string_lossy());
    // Never a whole drive or a shared program folder.
    (d.len() > 3 && !["program files\\", "program files (x86)\\", "games\\"].iter().any(|s| d.ends_with(&format!(":\\{s}")))).then_some(d)
}

/// The window is the picked program (by full path, by file name, or by family: an anti-cheat wrapper's twin).
pub fn is_pick(w: &WinInfo, p: &Prefs) -> bool {
    if p.pick.is_empty() && p.pick_path.is_empty() {
        return false;
    }
    let key = games::exe_key(&w.exe);
    let lower = w.exe.to_lowercase();
    (!p.pick_path.is_empty() && lower == p.pick_path.to_lowercase()) || (!p.pick.is_empty() && key == p.pick) || (!p.pick.is_empty() && family(&stem_of(&w.exe)) == family(&stem_of(p.pick)))
}

/// What makes window `w` a game. None: never one (the "never" list, the deny table, a helper),
/// or nothing counts while automatic detection is off. `front_long`: it has been in front for
/// `FRONT_SECS`.
pub fn evidence(w: &WinInfo, p: &Prefs, lib: &Library, front_long: bool) -> Option<Vec<Ev>> {
    let key = games::exe_key(&w.exe);
    if key.is_empty() || p.not_games.iter().any(|n| *n == key) {
        return None;
    }
    if is_pick(w, p) {
        return Some(vec![ev("pick", 1000)]);
    }
    let lower = w.exe.to_lowercase();
    let stem = stem_of(&w.exe);
    let big = w.minimized || (w.width >= MIN_W && w.height >= MIN_H);
    // A picked launcher hands over to the game it starts: the same install folder, or a game of its store.
    let pick_stem = stem_of(if p.pick_path.is_empty() { p.pick } else { p.pick_path });
    let handoff = !pick_stem.is_empty() && big && denied(&stem).is_none() && !HELPER_WORDS.iter().any(|h| stem.contains(h)) && {
        let by_root = pick_root(p.pick_path, lib).is_some_and(|r| lower.starts_with(&r));
        let by_store = launcher_store(&pick_stem).is_some_and(|s| lib.find(&w.exe).is_some_and(|i| i.store == s));
        by_root || by_store
    };
    if handoff {
        return Some(vec![ev("handoff", 950)]);
    }
    if !p.auto || !big {
        return None;
    }
    if p.games.iter().any(|g| *g == key) {
        return Some(vec![ev("marked", 500)]);
    }
    if denied(&stem).is_some() || lower.contains("\\launcher\\") || HELPER_WORDS.iter().any(|h| stem.contains(h)) {
        return None;
    }
    let mut out = Vec::new();
    if stem == "javaw" || stem == "java" {
        let mc = games::name_for(w, &[]) == "Minecraft";
        if !mc {
            return None;
        }
        out.push(ev("minecraft", 300));
    } else if games::known_exe_name(&stem).is_some() {
        out.push(ev("known", 300));
    }
    if let Some(i) = lib.find(&w.exe) {
        out.push(Ev { code: format!("library:{}", i.store), points: 260 });
    }
    if p.seen.iter().any(|g| !g.exe.is_empty() && g.name != games::DESKTOP && g.exe.to_lowercase() == lower) {
        out.push(ev("seen", 200));
    }
    if out.is_empty() && GAME_DIRS.iter().any(|d| lower.contains(d)) {
        out.push(ev("folder", 150));
    }
    if w.fullscreen {
        out.push(ev("fullscreen", 60));
    }
    if w.popup && !w.caption && !w.minimized && w.width >= 640 && w.height >= 480 {
        out.push(ev("gamewindow", 30));
    }
    if front_long && (w.fullscreen || w.popup) {
        out.push(ev("front", 30));
    }
    Some(out)
}

pub fn points(e: &[Ev]) -> i64 {
    e.iter().map(|x| x.points).sum()
}

// ---------------------------------------------------------------------------
// The decision.

/// Facts about this PC at the moment of the look.
#[derive(Clone, Debug)]
pub struct Env {
    /// This PC can capture one window (ffmpeg has Graphics Capture and the method is Graphics Capture).
    pub window_capture: bool,
    /// The configured method resolves to Desktop Duplication.
    pub dda: bool,
    /// The shell says an exclusive-fullscreen Direct3D program is in front (`win::d3d_fullscreen`).
    pub d3d_fullscreen: bool,
    /// Seconds on any steady clock (tests use a fake one).
    pub now: f64,
}

/// A change of target or of source is made once it has held this long (two looks), unless what is
/// recorded now is gone. Pausing is at once; resuming waits as long.
pub const HOLD_SECS: f64 = 4.0;

/// What the capture is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Source {
    /// The game's window (Graphics Capture by handle).
    Window,
    /// The game's monitor by Desktop Duplication (exclusive fullscreen, which window capture cannot see).
    MonitorDda,
    /// The game's monitor by the configured method (window capture refused, or not available).
    Monitor,
    /// Nothing is captured for now; the buffer is kept (minimized behind a monitor source, hidden, blocked).
    Park,
    /// No game: nothing is captured.
    Wait,
}

/// Why: what the UI's switch, its tooltip and the status line say, and what the log says once.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Why {
    /// "window" | "minimized" | "exclusive" | "window_failed" | "no_window_capture" | "excluded"
    /// | "hidden" | "paused_away" | "waiting_window" | "waiting_pick" | "waiting" | "auto_off" | "desktop"
    pub code: String,
    /// The game's name ("" when none).
    pub game: String,
    /// Index of the monitor recorded (monitor sources), as `win::monitors()` numbers them.
    pub monitor: u32,
    /// The evidence of the target ("pick", "library:steam", "fullscreen", ...).
    pub evidence: Vec<String>,
}

impl Why {
    /// The log's line (English).
    pub fn line(&self) -> String {
        let g = if self.game.is_empty() { "the game" } else { &self.game };
        let ev = if self.evidence.is_empty() { String::new() } else { format!(" [{}]", self.evidence.join(", ")) };
        let m = self.monitor + 1;
        let text = match self.code.as_str() {
            "window" => format!("recording {g}'s window"),
            "minimized" => format!("{g} is minimized; its window capture keeps the last picture"),
            "exclusive" => format!("{g} runs in exclusive fullscreen; recording its monitor {m} by Desktop Duplication instead"),
            "window_failed" => format!("{g}'s window gave no picture; recording its monitor {m} instead"),
            "no_window_capture" => format!("window capture is not available on this PC; recording {g}'s monitor {m}"),
            "excluded" => format!("{g} keeps its window out of screen capture; nothing is recorded"),
            "hidden" => format!("{g}'s window is hidden (another desktop or suspended); paused"),
            "paused_away" => format!("{g} is not in front; its monitor is not recorded meanwhile"),
            "waiting_window" => format!("waiting for {g}'s window"),
            "waiting_pick" => format!("waiting for {g} (your pick)"),
            "auto_off" => "automatic detection is off and no game is picked; waiting".to_string(),
            "desktop" => "desktop mode".to_string(),
            _ => "waiting for a game".to_string(),
        };
        format!("{text}{ev}")
    }
}

/// A program in the window list with what counts for it.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Cand {
    /// Index into the window list.
    #[serde(skip)]
    pub index: usize,
    pub exe: String,
    pub name: String,
    pub evidence: Vec<String>,
    pub score: i64,
    /// It counts as a game.
    pub game: bool,
}

/// What was decided before (kept by capmode.rs between looks).
#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// The window recorded: (hwnd, pid, exe key, name).
    pub target: Option<(u64, u32, String, String)>,
    pub source: Option<Source>,
    /// The reason last given (kept while a change waits to hold).
    pub code: String,
    /// Another window seen as the better choice, waiting to hold: (hwnd, since).
    pub pending_target: Option<(u64, f64)>,
    /// Another source wanted for the target, waiting to hold: (hwnd, source, reason, since).
    pub pending_source: Option<(u64, Source, String, f64)>,
    /// The process in front and since when.
    pub front: Option<(u32, f64)>,
    /// Windows whose capture gave no picture (`RecStatus::window_failed`, or no first frame).
    pub failed: Vec<u64>,
    /// Windows seen in exclusive fullscreen: their monitor is recorded by Desktop Duplication.
    pub exclusive: Vec<u64>,
}

pub struct Decision {
    /// Index into the window list of the window recorded (or followed while paused).
    pub window: Option<usize>,
    pub source: Source,
    pub why: Why,
    /// Every program that counts as a game, best first.
    pub candidates: Vec<Cand>,
}

/// The decision for this look. `alive`: whether a process still runs (a window being re-created).
pub fn decide(wins: &[WinInfo], lib: &Library, p: &Prefs, env: &Env, mem: &mut Memory, alive: &dyn Fn(u32) -> bool) -> Decision {
    let now = env.now;
    // Who is in front, and since when.
    let fg = wins.iter().find(|w| w.foreground).map(|w| w.pid);
    mem.front = match (fg, mem.front) {
        (Some(pid), Some((was, since))) if pid == was => Some((pid, since)),
        (Some(pid), _) => Some((pid, now)),
        (None, _) => None,
    };
    let front_long = |w: &WinInfo| w.foreground && mem.front.is_some_and(|(pid, since)| pid == w.pid && now - since >= FRONT_SECS);
    mem.failed.retain(|h| wins.iter().any(|w| w.hwnd == *h));
    mem.exclusive.retain(|h| wins.iter().any(|w| w.hwnd == *h));

    // Score every window.
    let cur_pid = mem.target.as_ref().map(|t| t.1);
    let mut scored: Vec<(usize, Vec<Ev>, i64)> = Vec::new();
    for (i, w) in wins.iter().enumerate() {
        let Some(e) = evidence(w, p, lib, front_long(w)) else { continue };
        let base = points(&e);
        let user = e.iter().any(|x| ["pick", "handoff", "marked"].contains(&x.code.as_str()));
        if base < GAME_MIN && !user {
            continue;
        }
        // A picked store launcher's own window gives way to the game it started.
        let launcher_pick = e.iter().any(|x| x.code == "pick") && launcherish(&w.exe);
        let rank = base - if launcher_pick { 200 } else { 0 } + if w.foreground { 200 } else { 0 } + if cur_pid == Some(w.pid) { 100 } else { 0 } - if w.minimized { 100 } else { 0 } - if w.cloaked { 300 } else { 0 };
        scored.push((i, e, rank));
    }
    scored.sort_by(|a, b| {
        let area = |i: usize| wins[i].width as u64 * wins[i].height as u64;
        b.2.cmp(&a.2).then(area(b.0).cmp(&area(a.0)))
    });
    let mut candidates: Vec<Cand> = Vec::new();
    for (i, e, rank) in &scored {
        let w = &wins[*i];
        let exe = games::exe_key(&w.exe);
        if candidates.iter().any(|c| c.exe == exe) {
            continue;
        }
        candidates.push(Cand { index: *i, exe, name: name_of(w, p.seen, lib), evidence: e.iter().map(|x| x.code.clone()).collect(), score: *rank, game: true });
    }

    // The pool the target comes from: with a pick, only the pick and what it hands over to.
    let picked = !p.pick.is_empty() || !p.pick_path.is_empty();
    let pool: Vec<&(usize, Vec<Ev>, i64)> = scored.iter().filter(|(_, e, _)| !picked || e.iter().any(|x| x.code == "pick" || x.code == "handoff")).collect();
    let best = pool.first().map(|(i, _, _)| *i);

    // Keep the current target while a different choice has not held for HOLD_SECS.
    let cur_idx = mem.target.as_ref().and_then(|t| wins.iter().position(|w| w.hwnd == t.0));
    let cur_in_pool = cur_idx.is_some_and(|ci| pool.iter().any(|(i, _, _)| *i == ci));
    let mut chosen = best;
    if let (Some(b), Some(t)) = (best, mem.target.clone()) {
        let w = &wins[b];
        if w.hwnd != t.0 {
            let recreated = cur_idx.is_none() && w.pid == t.1; // the same program's new window
            let to_pick = picked && pool.first().is_some_and(|(_, e, _)| e.iter().any(|x| x.code == "pick")) && games::exe_key(&w.exe) != t.2;
            if !(recreated || to_pick || (cur_idx.is_none() && !alive(t.1))) && (cur_in_pool || cur_idx.is_none()) {
                match mem.pending_target {
                    Some((h, since)) if h == w.hwnd && now - since >= HOLD_SECS => mem.pending_target = None,
                    Some((h, _)) if h == w.hwnd => chosen = cur_idx,
                    _ => {
                        mem.pending_target = Some((w.hwnd, now));
                        chosen = cur_idx;
                    }
                }
            } else {
                mem.pending_target = None;
            }
        } else {
            mem.pending_target = None;
        }
    }

    // No window to record.
    let Some(i) = chosen else {
        // The program recorded is still running but its window is gone for a moment (a map load,
        // a mode switch re-creating it): wait for it, keeping the buffer; never the desktop.
        if let Some(t) = mem.target.clone() {
            if alive(t.1) && !p.not_games.contains(&t.2) && (!picked || is_pick(&WinInfo { exe: t.2.clone(), ..Default::default() }, p)) && (p.auto || picked) {
                mem.source = Some(Source::Park);
                mem.code = "waiting_window".into();
                return Decision { window: None, source: Source::Park, why: Why { code: "waiting_window".into(), game: t.3.clone(), ..Default::default() }, candidates };
            }
        }
        mem.target = None;
        mem.source = Some(Source::Wait);
        mem.pending_target = None;
        mem.pending_source = None;
        let why = if picked {
            let name = p.seen.iter().find(|g| games::exe_key(&g.exe) == p.pick).map(|g| g.name.clone()).unwrap_or_else(|| stem_of(if p.pick.is_empty() { p.pick_path } else { p.pick }));
            Why { code: "waiting_pick".into(), game: name, ..Default::default() }
        } else if !p.auto {
            Why { code: "auto_off".into(), ..Default::default() }
        } else {
            Why { code: "waiting".into(), ..Default::default() }
        };
        mem.code = why.code.clone();
        return Decision { window: None, source: Source::Wait, why, candidates };
    };

    let w = &wins[i];
    let name = candidates.iter().find(|c| c.index == i || c.exe == games::exe_key(&w.exe)).map(|c| c.name.clone()).unwrap_or_else(|| name_of(w, p.seen, lib));
    let evidence: Vec<String> = scored.iter().find(|(j, _, _)| *j == i).map(|(_, e, _)| e.iter().map(|x| x.code.clone()).collect()).unwrap_or_default();
    let new_target = mem.target.as_ref().map(|t| t.0) != Some(w.hwnd);
    mem.target = Some((w.hwnd, w.pid, games::exe_key(&w.exe), name.clone()));

    // How to record it.
    if w.foreground && w.fullscreen && env.d3d_fullscreen && !mem.exclusive.contains(&w.hwnd) {
        mem.exclusive.push(w.hwnd);
    }
    if mem.failed.contains(&w.hwnd) && w.fullscreen && !mem.exclusive.contains(&w.hwnd) {
        mem.exclusive.push(w.hwnd); // a fullscreen window that gives window capture nothing: exclusive
    }
    let monitor_src = |why: &str, src: Source| (src, why.to_string());
    let (want, code) = if w.cloaked {
        monitor_src("hidden", Source::Park)
    } else if w.excluded {
        monitor_src("excluded", Source::Park)
    } else if !env.window_capture || mem.exclusive.contains(&w.hwnd) || mem.failed.contains(&w.hwnd) {
        let (src, code) = if mem.exclusive.contains(&w.hwnd) {
            (Source::MonitorDda, "exclusive")
        } else if !env.window_capture {
            (if env.dda { Source::MonitorDda } else { Source::Monitor }, "no_window_capture")
        } else {
            (Source::Monitor, "window_failed")
        };
        // A monitor shows whatever is on it: it is recorded only while the game is up and in front.
        if w.minimized || !w.foreground {
            (Source::Park, if w.minimized { "minimized".to_string() } else { "paused_away".to_string() })
        } else {
            (src, code.to_string())
        }
    } else if w.minimized {
        (Source::Window, "minimized".to_string())
    } else {
        (Source::Window, "window".to_string())
    };

    // A change of source holds HOLD_SECS before it is made, except pausing (at once), a new target
    // or the first decision. So a game alt-tabbed every second does not restart the capture each time.
    let mut source = want;
    let mut code = code;
    match mem.source.filter(|_| !new_target) {
        Some(prev) if prev != want && want != Source::Park && prev != Source::Wait => {
            let held = match &mem.pending_source {
                Some((h, s, _, since)) if *h == w.hwnd && *s == want => now - since >= HOLD_SECS,
                _ => {
                    mem.pending_source = Some((w.hwnd, want, code.clone(), now));
                    false
                }
            };
            if held {
                mem.pending_source = None;
            } else {
                // Until it holds, the source and the reason stay what they were.
                source = prev;
                if !mem.code.is_empty() {
                    code = mem.code.clone();
                }
            }
        }
        _ => mem.pending_source = None,
    }
    mem.source = Some(source);
    mem.code = code.clone();
    let monitor = w.monitor;
    Decision { window: Some(i), source, why: Why { code, game: name, monitor, evidence }, candidates }
}

/// The name a window's game is filed under: the known table and games seen before first (clip
/// names keep working as before), then the library's install name, then `games::name_for`.
pub fn name_of(w: &WinInfo, seen: &[Game], lib: &Library) -> String {
    let stem = stem_of(&w.exe);
    if games::known_exe_name(&stem).is_some() || stem == "javaw" || stem == "java" || seen.iter().any(|g| !g.exe.is_empty() && g.exe.eq_ignore_ascii_case(&w.exe)) {
        return games::name_for(w, seen);
    }
    if let Some(i) = lib.find(&w.exe) {
        let n = games::canonical(&i.name);
        if n != games::DESKTOP {
            return n;
        }
    }
    // The rest may read the program's version info: once per program, not on every look.
    use std::collections::HashMap;
    use std::sync::Mutex;
    static NAMES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
    let key = w.exe.to_lowercase();
    if let Some(n) = NAMES.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()) {
        return n;
    }
    let n = games::name_for(w, seen);
    NAMES.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, n.clone());
    n
}

// ---------------------------------------------------------------------------
// Self-test, run from `--capmode-test`: scenario tables on made-up windows and fixture files.

pub fn self_test(check: &mut dyn FnMut(&str, bool, String), dir: &Path) {
    test_library(check, dir);
    test_scenarios(check);
    test_debounce(check);
}

fn w(hwnd: u64, pid: u32, exe: &str) -> WinInfo {
    WinInfo { hwnd, pid, exe: exe.into(), width: 1920, height: 1080, caption: true, ..Default::default() }
}

fn env(now: f64) -> Env {
    Env { window_capture: true, dda: false, d3d_fullscreen: false, now }
}

fn test_library(check: &mut dyn FnMut(&str, bool, String), dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    let steam = dir.join("Steam");
    let other = dir.join("D_drive").join("SteamLibrary");
    let _ = std::fs::create_dir_all(steam.join("steamapps"));
    let _ = std::fs::create_dir_all(other.join("steamapps"));
    let vdf = format!(
        "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t\t\"apps\" {{ \"570\" \"1\" }}\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
        steam.to_string_lossy().replace('\\', "\\\\"),
        other.to_string_lossy().replace('\\', "\\\\")
    );
    let _ = std::fs::write(steam.join("steamapps").join("libraryfolders.vdf"), vdf);
    let _ = std::fs::write(steam.join("steamapps").join("appmanifest_570.acf"), "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n}\n");
    let _ = std::fs::write(other.join("steamapps").join("appmanifest_1145360.acf"), "\"AppState\" { \"appid\" \"1145360\" \"name\" \"Hades\" \"installdir\" \"Hades\" }");
    let _ = std::fs::write(other.join("steamapps").join("appmanifest_228980.acf"), "\"AppState\" { \"appid\" \"228980\" \"name\" \"Steamworks Common Redistributables\" \"installdir\" \"Steamworks Shared\" }");
    let _ = std::fs::write(other.join("steamapps").join("appmanifest_99.acf"), "\"AppState\" { \"appid\" \"99\" \"name\" \"Broken");
    let _ = std::fs::write(other.join("steamapps").join("appmanifest_98.acf"), [0xffu8, 0xfe, 0x00, 0x01]);
    let st = steam_installs(&steam);
    let names: Vec<&str> = st.iter().map(|i| i.name.as_str()).collect();
    check("library: Steam lists its own folder and a library on another drive", names.contains(&"Dota 2") && names.contains(&"Hades"), format!("{names:?}"));
    check("library: Steam's redistributables and broken manifests are left out", st.len() == 2, format!("{} installs", st.len()));
    let hades = st.iter().find(|i| i.name == "Hades");
    check("library: an install's folder is steamapps\\common\\<installdir>", hades.is_some_and(|i| i.dir.ends_with("\\steamlibrary\\steamapps\\common\\hades\\")), format!("{:?}", hades.map(|i| &i.dir)));
    check("library: the old libraryfolders format (numbered paths) is read too", steam_library_paths("\"LibraryFolders\" { \"TimeNextStatsReport\" \"1\" \"1\" \"E:\\\\Games\\\\Steam\" }") == ["E:\\Games\\Steam"], String::new());
    check("library: a missing Steam folder gives nothing, no error", steam_installs(&dir.join("nope")).is_empty(), String::new());

    let epic = dir.join("Manifests");
    let _ = std::fs::create_dir_all(&epic);
    let item = |name: &str, loc: &str, cats: &str| format!("{{\"DisplayName\":\"{name}\",\"InstallLocation\":\"{}\",\"AppCategories\":[{cats}],\"LaunchExecutable\":\"x.exe\"}}", loc.replace('\\', "\\\\"));
    let _ = std::fs::write(epic.join("A.item"), item("Fortnite", r"C:\Program Files\Epic Games\Fortnite", "\"public\",\"games\",\"applications\""));
    let _ = std::fs::write(epic.join("B.item"), item("Unreal Engine", r"C:\Program Files\Epic Games\UE_5.4", "\"public\",\"engines\""));
    let _ = std::fs::write(epic.join("C.item"), "{ not json");
    let _ = std::fs::write(epic.join("D.item"), item("Rocket League", r"D:\Epic\rocketleague", "\"games\""));
    let ep = epic_installs(&epic);
    let names: Vec<&str> = ep.iter().map(|i| i.name.as_str()).collect();
    check("library: Epic games from the .item manifests; the engine and a broken file are left out", names.len() == 2 && names.contains(&"Fortnite") && names.contains(&"Rocket League"), format!("{names:?}"));

    let riot = riot_installs(r#"{"associated_client":{"C:/Riot Games/VALORANT/live/":"C:/Riot Games/Riot Client/RiotClientServices.exe","D:/Riot Games/League of Legends/":"x"},"rc_default":"x"}"#);
    check("library: Riot's installs, named after their folder", riot.len() == 2 && riot[0].name == "VALORANT" && riot[0].dir == "c:\\riot games\\valorant\\live\\" && riot[1].name == "League of Legends", format!("{riot:?}"));
    check("library: a broken Riot file gives nothing", riot_installs("{").is_empty(), String::new());

    let drive = dir.join("X");
    let _ = std::fs::create_dir_all(drive.join("XboxGames").join("Halo Infinite").join("Content"));
    let _ = std::fs::create_dir_all(drive.join("XboxGames").join("GameSave"));
    let _ = std::fs::create_dir_all(drive.join("XboxGames").join("Minecraft Launcher").join("Content"));
    let mut root = b"RGBX".to_vec();
    root.extend_from_slice(&1u32.to_le_bytes());
    for u in "XboxGames".encode_utf16().chain(Some(0)) {
        root.extend_from_slice(&u.to_le_bytes());
    }
    let _ = std::fs::write(drive.join(".GamingRoot"), &root);
    let xb = xbox_installs(&drive);
    check("library: Xbox / Game Pass games from the drive's .GamingRoot (its saves folder and launchers left out)", xb.len() == 1 && xb[0].name == "Halo Infinite" && xb[0].store == "xbox", format!("{xb:?}"));
    check("library: a .GamingRoot that is not one is ignored", gaming_root_folders(b"hello world!").is_empty(), String::new());
    let un = vec![("Blizzard Entertainment".to_string(), "Overwatch".to_string(), r"C:\Program Files (x86)\Overwatch".to_string()), ("Blizzard Entertainment".into(), "Battle.net".into(), r"C:\Program Files (x86)\Battle.net".into()), ("Valve".into(), "x".into(), r"C:\x".into())];
    let bz = uninstall_installs(&un, "Blizzard Entertainment", "blizzard", &["Battle.net"]);
    check("library: Blizzard games from uninstall entries, not Battle.net itself", bz.len() == 1 && bz[0].name == "Overwatch", format!("{bz:?}"));

    let lib = Library { installs: [st, ep].concat() };
    let exe = format!(r"{}\steamapps\common\Hades\x64\Hades.exe", other.to_string_lossy());
    check("library: an exe inside an install is found, with the install's name", lib.find(&exe).is_some_and(|i| i.name == "Hades" && i.store == "steam"), String::new());
    check("library: an exe outside every install is not", lib.find(r"C:\Tools\emu.exe").is_none(), String::new());
    let _ = std::fs::remove_dir_all(dir);
}

fn test_scenarios(check: &mut dyn FnMut(&str, bool, String)) {
    let lib = Library {
        installs: vec![
            Install { store: "steam", name: "Hades".into(), dir: r"d:\steamlibrary\steamapps\common\hades\".into() },
            Install { store: "epic", name: "Fortnite".into(), dir: r"c:\program files\epic games\fortnite\".into() },
            Install { store: "steam", name: "Some Indie".into(), dir: r"e:\games\indie\".into() },
        ],
    };
    let alive = |_: u32| true;
    let dead = |_: u32| false;
    let run = |wins: &[WinInfo], p: &Prefs, e: &Env, mem: &mut Memory| decide(wins, &lib, p, e, mem, &alive);
    let chrome = WinInfo { fullscreen: true, foreground: true, popup: true, caption: false, ..w(1, 10, r"C:\Program Files\Google\Chrome\Application\chrome.exe") };
    let emu = w(2, 11, r"C:\Tools\MyEmu\emu.exe");
    let pick_emu = Prefs { pick: "emu.exe", pick_path: r"c:\tools\myemu\emu.exe", ..Prefs::none() };

    // 1. The manual pick vs a fullscreen browser in front.
    let mut m = Memory::default();
    let d = run(&[chrome.clone(), emu.clone()], &pick_emu, &env(0.0), &mut m);
    check("scenario: the pick wins over a fullscreen browser in front", d.window == Some(1) && d.source == Source::Window && d.why.code == "window" && d.why.evidence == ["pick"], format!("{:?} {:?}", d.window, d.why));
    // 2. The pick by path after its file name changed case, and by name with no path (0.1.40 settings).
    let old = Prefs { pick: "emu.exe", ..Prefs::none() };
    check("scenario: a 0.1.40 pick (file name only) still finds its program", run(&[emu.clone()], &old, &env(0.0), &mut Memory::default()).window == Some(0), String::new());
    let moved = w(3, 12, r"C:\Tools\MyEmu-2.1\emu.exe");
    check("scenario: the pick follows its program after an update moved it (same file name)", run(&[moved], &pick_emu, &env(0.0), &mut Memory::default()).window == Some(0), String::new());
    // 3. An anti-cheat twin of the picked file.
    let be = w(4, 13, r"D:\Games\Shooter\Shooter_BE.exe");
    let pick_shooter = Prefs { pick: "shooter.exe", pick_path: r"d:\games\shooter\shooter.exe", ..Prefs::none() };
    check("scenario: the pick finds the game started as its anti-cheat twin (Shooter_BE.exe)", run(&[be.clone()], &pick_shooter, &env(0.0), &mut Memory::default()).window == Some(0), String::new());
    check("scenario: family() folds anti-cheat suffixes", family("FortniteClient-Win64-Shipping_EAC_EOS") == "fortniteclient-win64-shipping" && family("game_be.exe") == "game" && family("rust") == "rust", String::new());
    // 4. A launcher picked: its game, from the same install folder, takes over.
    let launcher = WinInfo { width: 1000, height: 640, ..w(5, 14, r"E:\Games\Indie\Launcher\IndieLauncher.exe") };
    let game = WinInfo { popup: true, caption: false, fullscreen: true, ..w(6, 15, r"E:\Games\Indie\Bin\Indie.exe") };
    let pick_l = Prefs { pick: "indielauncher.exe", pick_path: r"e:\games\indie\launcher\indielauncher.exe", ..Prefs::none() };
    let mut m = Memory::default();
    let d = run(&[launcher.clone()], &pick_l, &env(0.0), &mut m);
    check("scenario: a picked launcher is recorded while it is all there is", d.window == Some(0), format!("{:?}", d.why));
    let d = run(&[launcher.clone(), game.clone()], &pick_l, &env(2.0), &mut m);
    let held = d.window == Some(0);
    let d = run(&[launcher.clone(), game.clone()], &pick_l, &env(6.5), &mut m);
    check("scenario: the game started from the picked launcher's folder takes over (after the hold)", held && d.window == Some(1) && d.why.evidence == ["handoff"], format!("{:?} {:?}", d.window, d.why));
    let steam_ui = WinInfo { width: 1200, height: 800, ..w(7, 16, r"C:\Program Files (x86)\Steam\steam.exe") };
    let hades = WinInfo { foreground: true, ..w(8, 17, r"D:\SteamLibrary\steamapps\common\Hades\x64\Hades.exe") };
    let pick_steam = Prefs { pick: "steam.exe", pick_path: r"c:\program files (x86)\steam\steam.exe", ..Prefs::none() };
    let d = run(&[steam_ui.clone(), hades.clone()], &pick_steam, &env(0.0), &mut Memory::default());
    check("scenario: Steam picked: the Steam game it started is recorded, not Steam's window", d.window == Some(1) && d.why.game == "Hades", format!("{:?} {:?}", d.window, d.why));
    // 5. Launcher window vs game window of the same install folder, no pick.
    let epic_launch = WinInfo { width: 900, height: 600, ..w(9, 18, r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteLauncher.exe") };
    let fort = WinInfo { fullscreen: true, popup: true, caption: false, ..w(10, 19, r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient-Win64-Shipping.exe") };
    let d = run(&[epic_launch.clone(), fort.clone()], &Prefs::none(), &env(0.0), &mut Memory::default());
    check("scenario: the game's window wins over the launcher of the same install", d.window == Some(1) && d.why.game == "Fortnite", format!("{:?} {:?}", d.window, d.why));
    check("scenario: a launcher alone is not a game", run(&[epic_launch.clone()], &Prefs::none(), &env(0.0), &mut Memory::default()).window.is_none(), String::new());
    // 6. Library evidence names an unknown exe.
    let indie = w(11, 20, r"E:\Games\Indie\Bin\Indie.exe");
    let d = run(&[indie.clone()], &Prefs::none(), &env(0.0), &mut Memory::default());
    check("scenario: an exe inside a Steam install is a game with the install's name", d.window == Some(0) && d.why.game == "Some Indie" && d.why.evidence.contains(&"library:steam".to_string()), format!("{:?}", d.why));
    // 7. Recreated window: a new handle of the same program is taken at once.
    let valo = WinInfo { foreground: true, ..w(20, 30, r"C:\Riot Games\VALORANT\live\VALORANT.exe") };
    let mut m = Memory::default();
    let _ = run(&[valo.clone()], &Prefs::none(), &env(0.0), &mut m);
    let gone = run(&[], &Prefs::none(), &env(2.0), &mut m);
    check("scenario: its window gone while it runs: paused, waiting for its window, never the desktop", gone.window.is_none() && gone.source == Source::Park && gone.why.code == "waiting_window" && gone.why.game == "Valorant", format!("{:?} {:?}", gone.source, gone.why));
    let again = WinInfo { hwnd: 21, ..valo.clone() };
    let d = run(&[again.clone()], &Prefs::none(), &env(3.0), &mut m);
    check("scenario: the game's re-created window (new handle) is captured at once", d.window == Some(0) && d.source == Source::Window && m.target.as_ref().is_some_and(|t| t.0 == 21), format!("{:?}", d.why));
    let ended = decide(&[], &lib, &Prefs::none(), &env(5.0), &mut m, &dead);
    check("scenario: its process ended: no game, waiting", ended.source == Source::Wait && ended.why.code == "waiting", format!("{:?}", ended.why));
    // 8. Minimized.
    let mut m = Memory::default();
    let _ = run(&[valo.clone()], &Prefs::none(), &env(0.0), &mut m);
    let mini = WinInfo { minimized: true, foreground: false, width: 0, height: 0, ..valo.clone() };
    let d = run(&[mini.clone()], &Prefs::none(), &env(2.0), &mut m);
    check("scenario: minimized (window capture): kept, it says minimized", d.window == Some(0) && d.source == Source::Window && d.why.code == "minimized", format!("{:?} {:?}", d.source, d.why));
    // 9. Cloaked store app.
    let uwp = WinInfo { cloaked: true, hosted: true, ..w(30, 40, r"C:\Program Files\WindowsApps\Microsoft.MinecraftUWP_1.21\Minecraft.Windows.exe") };
    let mut m = Memory::default();
    let d = run(&[WinInfo { cloaked: false, ..uwp.clone() }], &Prefs::none(), &env(0.0), &mut m);
    let shown = d.window == Some(0) && d.source == Source::Window;
    let d = run(&[uwp.clone()], &Prefs::none(), &env(2.0), &mut m);
    check("scenario: a store app's window (its own exe, not ApplicationFrameHost) is found; cloaked it pauses and says so", shown && d.source == Source::Park && d.why.code == "hidden", format!("{:?} {:?}", d.source, d.why));
    let frame = WinInfo { foreground: true, fullscreen: true, ..w(31, 41, r"C:\Windows\System32\ApplicationFrameHost.exe") };
    check("scenario: ApplicationFrameHost itself is never a game", run(&[frame], &Prefs::none(), &env(0.0), &mut Memory::default()).window.is_none(), String::new());
    // 10. Two monitors, the game on the second.
    let second = WinInfo { monitor: 1, fullscreen: true, foreground: true, ..valo.clone() };
    let d = run(&[second.clone()], &Prefs::none(), &Env { d3d_fullscreen: true, ..env(0.0) }, &mut Memory::default());
    check("scenario: exclusive fullscreen on monitor 2: its monitor 2 by Desktop Duplication", d.source == Source::MonitorDda && d.why.code == "exclusive" && d.why.monitor == 1, format!("{:?} {:?}", d.source, d.why));
    check("scenario: the log line says so", d.why.line().contains("exclusive fullscreen") && d.why.line().contains("monitor 2"), d.why.line());
    // 11. Exclusive fullscreen flagged by a window capture that gave nothing.
    let fs = WinInfo { fullscreen: true, foreground: true, ..valo.clone() };
    let mut m = Memory::default();
    let d0 = run(&[fs.clone()], &Prefs::none(), &env(0.0), &mut m);
    m.failed.push(fs.hwnd);
    let d1 = run(&[fs.clone()], &Prefs::none(), &env(14.0), &mut m);
    let d2 = run(&[fs.clone()], &Prefs::none(), &env(18.5), &mut m);
    check("scenario: a fullscreen window whose capture gave no picture becomes exclusive: Desktop Duplication (after the hold)", d0.source == Source::Window && d1.source == Source::Window && d2.source == Source::MonitorDda && d2.why.code == "exclusive", format!("{:?} {:?} {:?}", d0.source, d1.source, d2.source));
    // 12. A windowed game whose window capture is refused: its monitor, said so, only while in front.
    let windowed = WinInfo { foreground: true, width: 1600, height: 900, ..valo.clone() };
    let mut m = Memory { failed: vec![windowed.hwnd], ..Default::default() };
    let d = run(&[windowed.clone()], &Prefs::none(), &env(0.0), &mut m);
    check("scenario: window capture refused on a windowed game: its monitor by the configured method, said so", d.source == Source::Monitor && d.why.code == "window_failed", format!("{:?} {:?}", d.source, d.why));
    let away = WinInfo { foreground: false, ..windowed.clone() };
    let d = run(&[away.clone()], &Prefs::none(), &env(1.0), &mut m);
    check("scenario: ... and paused at once when the game leaves the front (the desktop is never recorded)", d.source == Source::Park && d.why.code == "paused_away", format!("{:?} {:?}", d.source, d.why));
    // 13. No window capture on this PC (Desktop Duplication chosen): the monitor, said so.
    let d = run(&[fs.clone()], &Prefs::none(), &Env { window_capture: false, dda: true, ..env(0.0) }, &mut Memory::default());
    check("scenario: no window capture here: the game's monitor by Desktop Duplication, said so", d.source == Source::MonitorDda && d.why.code == "no_window_capture", format!("{:?} {:?}", d.source, d.why));
    // 14. Excluded from capture.
    let ex = WinInfo { excluded: true, ..valo.clone() };
    let d = run(&[ex], &Prefs::none(), &env(0.0), &mut Memory::default());
    check("scenario: a game that keeps itself out of capture: paused and said so, not the monitor", d.source == Source::Park && d.why.code == "excluded", format!("{:?}", d.why));
    // 15. The deny list and the user's never list.
    let denied_wins = [
        WinInfo { fullscreen: true, foreground: true, ..w(40, 50, r"C:\Users\me\AppData\Local\Discord\app-1.0\Discord.exe") },
        WinInfo { fullscreen: true, ..w(41, 51, r"C:\Program Files\obs-studio\bin\64bit\obs64.exe") },
        WinInfo { fullscreen: true, ..w(42, 52, r"C:\Windows\explorer.exe") },
        WinInfo { fullscreen: true, ..w(43, 53, r"D:\SteamLibrary\steamapps\common\wallpaper_engine\bin\wallpaperui.exe") },
        WinInfo { fullscreen: true, ..w(44, 54, r"C:\Users\me\AppData\Local\Programs\Spotifast\spotifast.exe") },
        WinInfo { fullscreen: true, ..w(45, 55, r"C:\Program Files\VideoLAN\VLC\vlc.exe") },
        WinInfo { fullscreen: true, ..w(46, 56, r"C:\Program Files\Glint\glint.exe") },
        WinInfo { fullscreen: true, ..w(47, 57, r"C:\Windows\System32\GameBar.exe") },
    ];
    check("scenario: launchers, chat, capture tools, shell, wallpaper, players, Glint and the Game Bar are never games", run(&denied_wins, &Prefs::none(), &env(0.0), &mut Memory::default()).window.is_none(), String::new());
    check("scenario: every deny group is in one table", ["launcher", "browser", "chat", "capture", "overlay", "player", "editor", "tool", "shell"].iter().all(|k| DENY.iter().any(|(g, _)| g == k)) && denied("discord") == Some("chat") && denied("hades").is_none(), String::new());
    let never = ["valorant.exe".to_string()];
    let d = run(&[fs.clone()], &Prefs { not_games: &never, ..Prefs::none() }, &env(0.0), &mut Memory::default());
    check("scenario: \"never treat this as a game\" holds even fullscreen in front", d.window.is_none(), String::new());
    // 16. Fullscreen alone, in the background, is not enough; in front for a while it is.
    let unknown = WinInfo { fullscreen: true, popup: true, caption: false, ..w(50, 60, r"C:\Indie\Thing\thing.exe") };
    check("scenario: an unknown fullscreen game-like window counts (fullscreen + swap-chain window)", run(&[unknown.clone()], &Prefs::none(), &env(0.0), &mut Memory::default()).window == Some(0), String::new());
    let app = WinInfo { fullscreen: true, ..w(51, 61, r"C:\Apps\Reader\reader.exe") };
    let mut m = Memory::default();
    let a = run(&[app.clone()], &Prefs::none(), &env(0.0), &mut m).window;
    let front = WinInfo { foreground: true, ..app.clone() };
    let b = run(&[front.clone()], &Prefs::none(), &env(1.0), &mut m).window;
    let c = run(&[front.clone()], &Prefs::none(), &env(12.0), &mut m).window;
    check("scenario: a captioned app filling the screen is a game only after FRONT_SECS in front", a.is_none() && b.is_none() && c == Some(0), format!("{a:?} {b:?} {c:?}"));
    // 17. Detection off and the pick away.
    let d = run(&[fs.clone()], &Prefs { auto: false, ..Prefs::none() }, &env(0.0), &mut Memory::default());
    check("scenario: detection off and nothing picked: waiting, said so", d.source == Source::Wait && d.why.code == "auto_off", format!("{:?}", d.why));
    let d = run(&[fs.clone()], &pick_emu, &env(0.0), &mut Memory::default());
    check("scenario: the pick is not running: waiting for it, never another game", d.window.is_none() && d.why.code == "waiting_pick" && d.why.game == "emu", format!("{:?}", d.why));
    // 18. The splash first, then the real window of the picked program.
    let splash = WinInfo { width: 480, height: 270, popup: true, caption: false, ..w(60, 70, r"C:\Tools\MyEmu\emu.exe") };
    let main = WinInfo { hwnd: 61, width: 1920, height: 1080, ..splash.clone() };
    let mut m = Memory::default();
    let _ = run(&[splash.clone()], &pick_emu, &env(0.0), &mut m);
    let d = run(&[main.clone()], &pick_emu, &env(2.0), &mut m);
    check("scenario: the picked program's splash, then its real window: the capture moves to the real window", d.window == Some(0) && m.target.as_ref().is_some_and(|t| t.0 == 61), String::new());
    // 19. Candidates for the menu.
    let d = run(&[chrome.clone(), indie.clone(), fs.clone()], &Prefs::none(), &env(0.0), &mut Memory::default());
    check("scenario: the menu's candidates are the games with their evidence, best first, the browser left out", d.candidates.len() == 2 && d.candidates[0].exe == "valorant.exe" && d.candidates[0].evidence.contains(&"known".to_string()) && d.candidates.iter().all(|c| c.exe != "chrome.exe"), format!("{:?}", d.candidates.iter().map(|c| (&c.exe, &c.evidence)).collect::<Vec<_>>()));
}

fn test_debounce(check: &mut dyn FnMut(&str, bool, String)) {
    let lib = Library::default();
    let alive = |_: u32| true;
    let a = WinInfo { foreground: true, ..w(100, 1, r"C:\Riot Games\VALORANT\live\VALORANT.exe") };
    let b = w(200, 2, r"D:\SteamLibrary\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe");
    let fg = |x: &WinInfo, on: bool| WinInfo { foreground: on, ..x.clone() };
    // Alt-tab between two games every second for 20 s: the target never flips.
    let mut m = Memory::default();
    let mut flips = 0;
    let mut last = None;
    for k in 0..20 {
        let front_a = k % 2 == 0;
        let d = decide(&[fg(&a, front_a), fg(&b, !front_a)], &lib, &Prefs::none(), &env(k as f64), &mut m, &alive);
        let t = d.window.map(|i| [100u64, 200][i]);
        if last.is_some() && t != last {
            flips += 1;
        }
        last = t;
    }
    check("debounce: alt-tab between two games every second never moves the capture", flips == 0 && last == Some(100), format!("{flips} flips"));
    // The other game held in front for the hold time: the capture moves once.
    let mut m = Memory::default();
    let _ = decide(&[fg(&a, true), fg(&b, false)], &lib, &Prefs::none(), &env(0.0), &mut m, &alive);
    let t1 = decide(&[fg(&a, false), fg(&b, true)], &lib, &Prefs::none(), &env(2.0), &mut m, &alive).window;
    let t2 = decide(&[fg(&a, false), fg(&b, true)], &lib, &Prefs::none(), &env(2.0 + HOLD_SECS - 0.1), &mut m, &alive).window;
    let t3 = decide(&[fg(&a, false), fg(&b, true)], &lib, &Prefs::none(), &env(2.0 + HOLD_SECS), &mut m, &alive).window;
    check("debounce: another game played in front takes over only after it held HOLD_SECS (fake clock)", t1 == Some(0) && t2 == Some(0) && t3 == Some(1), format!("{t1:?} {t2:?} {t3:?}"));
    // A monitor source with the game alt-tabbed every second: paused at once, resumed only after the hold.
    let mut m = Memory { failed: vec![100], ..Default::default() };
    let win_a = WinInfo { width: 1600, height: 900, ..a.clone() };
    let mut sources = Vec::new();
    for k in 0..10 {
        let d = decide(&[fg(&win_a, k % 2 == 0)], &lib, &Prefs::none(), &env(k as f64), &mut m, &alive);
        sources.push(d.source);
    }
    let restarts = sources.windows(2).filter(|p| p[0] != p[1]).count();
    check("debounce: a monitor source alt-tabbed every second pauses once and does not restart every second", sources[0] == Source::Monitor && restarts == 1 && sources[1..].iter().all(|s| *s == Source::Park), format!("{sources:?}"));
    let mut later = Vec::new();
    for k in 10..16 {
        later.push(decide(&[fg(&win_a, true)], &lib, &Prefs::none(), &env(k as f64), &mut m, &alive).source);
    }
    check("debounce: back in front for good: the monitor capture resumes after the hold", later[0] == Source::Park && later.last() == Some(&Source::Monitor), format!("{later:?}"));
    // The pick always switches at once (the user's own choice).
    let mut m = Memory::default();
    let _ = decide(&[fg(&a, true), fg(&b, false)], &lib, &Prefs::none(), &env(0.0), &mut m, &alive);
    let p = Prefs { pick: "cs2.exe", ..Prefs::none() };
    let d = decide(&[fg(&a, true), fg(&b, false)], &lib, &p, &env(0.5), &mut m, &alive);
    check("debounce: a new pick is followed at once", d.window == Some(1) && d.why.evidence == ["pick"], format!("{:?}", d.why));
}
