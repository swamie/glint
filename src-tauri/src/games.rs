//! Game detection: decide whether the foreground window is a game, give it a
//! clean name, and remember each game seen (with its icon) in `games.json`.
use crate::util;
use crate::win::{self, Foreground};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Clips taken with no game running are filed under this name.
pub const DESKTOP: &str = "Desktop";

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Game {
    pub name: String,
    /// Executable the game was last seen running from.
    pub exe: String,
    /// Icon image file ("" = none yet): the exe's own icon, or one the user chose.
    pub icon: String,
    /// The user picked the icon: never replace it with the exe's.
    pub custom_icon: bool,
}

/// Executable name (lower case, no ".exe") -> game name, for games whose
/// executable or version info does not carry a usable name.
const KNOWN: &[(&str, &str)] = &[
    ("valorant-win64-shipping", "Valorant"),
    ("valorant", "Valorant"),
    ("fortniteclient-win64-shipping", "Fortnite"),
    ("robloxplayerbeta", "Roblox"),
    ("osu!", "osu!"),
    ("league of legends", "League of Legends"),
    ("cs2", "Counter-Strike 2"),
    ("r5apex", "Apex Legends"),
    ("r5apex_dx12", "Apex Legends"),
    ("overwatch", "Overwatch"),
    ("deadlock", "Deadlock"),
    ("project8", "Deadlock"),
    ("albion-online", "Albion Online"),
    ("gta5", "Grand Theft Auto V"),
    ("gta5_enhanced", "Grand Theft Auto V"),
    ("rocketleague", "Rocket League"),
    ("rainbowsix", "Rainbow Six Siege"),
    ("rainbowsix_vulkan", "Rainbow Six Siege"),
    ("eldenring", "Elden Ring"),
    ("minecraft.windows", "Minecraft"),
    ("dota2", "Dota 2"),
    ("destiny2", "Destiny 2"),
    ("cod", "Call of Duty"),
    ("tslgame", "PUBG"),
    ("rustclient", "Rust"),
    ("escapefromtarkov", "Escape from Tarkov"),
    ("helldivers2", "Helldivers 2"),
    ("marvel-win64-shipping", "Marvel Rivals"),
    ("discovery", "THE FINALS"),
    ("pioneergame", "ARC Raiders"),
    ("genshinimpact", "Genshin Impact"),
    ("starrail", "Honkai: Star Rail"),
    ("zenlesszonezero", "Zenless Zone Zero"),
    ("wow", "World of Warcraft"),
    ("hearthstone", "Hearthstone"),
    ("ffxiv_dx11", "Final Fantasy XIV"),
    ("warframe.x64", "Warframe"),
    ("pathofexile", "Path of Exile"),
    ("pathofexile_x64", "Path of Exile"),
    ("pathofexilesteam", "Path of Exile"),
];

/// Programs that are never games, even fullscreen or inside a game library.
const NOT_GAMES: &[&str] = &[
    "explorer", "searchhost", "shellexperiencehost", "startmenuexperiencehost", "applicationframehost", "lockapp", "textinputhost", "dwm",
    "chrome", "msedge", "firefox", "opera", "opera_gx", "brave", "vivaldi", "arc", "zen", "iexplore", "msedgewebview2",
    "discord", "discordptb", "discordcanary", "spotify", "slack", "teams", "ms-teams", "zoom", "telegram", "whatsapp", "signal",
    "steam", "steamwebhelper", "epicgameslauncher", "battle.net", "riotclientservices", "riotclientux", "leagueclient", "leagueclientux",
    "eadesktop", "origin", "upc", "ubisoftconnect", "galaxyclient", "playnite.desktopapp", "xboxpcapp", "gamebar", "gamingservices",
    "vlc", "mpv", "mpc-hc64", "mpc-hc", "wmplayer", "video.ui", "microsoft.media.player", "potplayermini64", "photos",
    "obs64", "obs", "medal", "medalencoder", "glint", "kite", "nvidia app", "nvidia share", "geforce experience", "streamlabs obs",
    "code", "devenv", "rider64", "idea64", "notepad", "notepad++", "windowsterminal", "powershell", "pwsh", "cmd", "conhost", "claude",
    "wallpaper32", "wallpaper64", "wallpaperservice32", "vrserver", "vrmonitor", "steamvr", "blender", "photoshop", "premiere pro", "afterfx", "resolve",
    "winword", "excel", "powerpnt", "outlook", "acrobat", "taskmgr", "mmc", "regedit", "systemsettings", "snippingtool",
];

/// More programs that are never games: launchers of Minecraft clients, mod managers, VPNs and remote-desktop tools.
const NOT_GAMES_MORE: &[&str] = &[
    "minecraftlauncher", "minecraft launcher", "lunar client", "badlion client", "feather launcher", "prismlauncher", "multimc", "curseforge",
    "modrinth app", "atlauncher", "gdlauncher", "tlauncher", "overwolf", "expressvpn", "expressvpn.vpn.ui", "nordvpn", "protonvpn", "surfshark",
    "mullvad vpn", "wireguard", "openvpn-gui", "anydesk", "teamviewer", "rustdesk", "7zfm", "winrar", "qbittorrent", "everything", "sharex", "lightshot",
];

/// Names that mean "no game", wherever they come from: other clippers file their desktop
/// captures under these, and NVIDIA names a clip after whatever app was in front.
const NOT_GAME_NAMES: &[&str] = &[
    "desktop", "screen recording", "screen capture", "screen", "desktop recording", "base profile", "windows", "explorer", "windows explorer",
    "file explorer", "expressvpn", "nordvpn", "proton vpn", "protonvpn", "surfshark", "google chrome", "chrome", "microsoft edge", "edge", "firefox",
    "mozilla firefox", "opera", "opera gx", "brave", "discord", "spotify", "steam", "epic games launcher", "battle.net", "riot client", "ea app",
    "ubisoft connect", "xbox", "medal", "medal.tv", "obs studio", "obs", "nvidia app", "geforce experience", "nvidia geforce experience",
    "visual studio code", "notepad", "task manager", "settings", "photos", "vlc media player", "vlc", "minecraft launcher", "lunar client launcher",
];

/// One game, several names: the name on the left (lower case) is filed under the name on the right.
/// Minecraft's many faces come first: imports from other clippers name it after the client or after Java.
const ALIASES: &[(&str, &str)] = &[
    ("minecraft java edition", "Minecraft"), ("minecraft: java edition", "Minecraft"), ("minecraft java", "Minecraft"), ("minecraft bedrock", "Minecraft"),
    ("minecraft for windows", "Minecraft"), ("minecraft windows 10 edition", "Minecraft"), ("lunar client", "Minecraft"), ("lunarclient", "Minecraft"),
    ("badlion client", "Minecraft"), ("badlion minecraft client", "Minecraft"), ("feather client", "Minecraft"), ("labymod", "Minecraft"),
    ("javaw", "Minecraft"), ("java", "Minecraft"), ("java(tm) platform se binary", "Minecraft"), ("openjdk platform binary", "Minecraft"),
    ("overwatch 2", "Overwatch"), ("overwatch®", "Overwatch"), ("valorant", "Valorant"), ("cs2", "Counter-Strike 2"), ("counter-strike 2", "Counter-Strike 2"),
    ("counter strike 2", "Counter-Strike 2"), ("league of legends (tm) client", "League of Legends"), ("lol", "League of Legends"),
    ("tom clancy's rainbow six siege", "Rainbow Six Siege"), ("rainbow six siege x", "Rainbow Six Siege"), ("gta v", "Grand Theft Auto V"),
    ("gta 5", "Grand Theft Auto V"), ("grand theft auto v enhanced", "Grand Theft Auto V"), ("fortnite", "Fortnite"), ("roblox", "Roblox"),
    ("apex legends", "Apex Legends"), ("playerunknown's battlegrounds", "PUBG"), ("pubg: battlegrounds", "PUBG"), ("the finals", "THE FINALS"),
    ("rocket league", "Rocket League"), ("osu!lazer", "osu!"), ("osu", "osu!"), ("risk of rain 2", "Risk of Rain 2"), ("deadlock", "Deadlock"),
];

/// The one name a game is filed under. Different sources call the same game different
/// things ("Overwatch 2" from NVIDIA, "Overwatch" from the running exe), and some of
/// their "games" are not games at all ("Screen Recording", "ExpressVPN"): those become `DESKTOP`.
pub fn canonical(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();
    // "Minecraft 1.8.9", "Minecraft* 1.21.4", "Lunar Client (1.8.9-abc/master)": the window title used as a name.
    if lower.starts_with("minecraft") && !["dungeons", "legends", "story mode", "launcher"].iter().any(|k| lower.contains(k)) {
        return "Minecraft".into();
    }
    if (lower.starts_with("lunar client") || lower.starts_with("badlion")) && !lower.contains("launcher") {
        return "Minecraft".into();
    }
    if let Some((_, to)) = ALIASES.iter().find(|(from, _)| *from == lower) {
        return to.to_string();
    }
    if trimmed.is_empty() || NOT_GAME_NAMES.contains(&lower.as_str()) || NOT_GAMES.contains(&lower.as_str()) || NOT_GAMES_MORE.contains(&lower.as_str()) {
        return DESKTOP.into();
    }
    trimmed.to_string()
}

/// Words before "Edition" that do not change which game it is ("Deluxe Edition", "Game of the Year Edition").
const EDITION_WORDS: &[&str] = &[
    "standard", "deluxe", "gold", "ultimate", "definitive", "complete", "goty", "special", "premium", "collectors", "anniversary", "digital", "enhanced",
    "legendary", "year", "launch", "founders", "champions", "cross", "gen", "pc", "windows", "steam",
];

/// A game name reduced to what tells games apart: lower case, no "™ ® ©", apostrophes
/// dropped, other punctuation as spaces, "Tom Clancy's" in front and an edition at the
/// end left out. "Tom Clancy's Rainbow Six® Siege" and "Rainbow Six Siege" give the same key.
pub fn name_key(name: &str) -> String {
    let lower = name.to_lowercase().replace("(tm)", " ").replace("(r)", " ").replace('&', " and ");
    let mut clean = String::with_capacity(lower.len());
    for c in lower.chars() {
        match c {
            '\'' | '\u{2019}' | '\u{2018}' | '`' | '\u{2122}' | '\u{00ae}' | '\u{00a9}' => {}
            c if c.is_alphanumeric() => clean.push(c),
            _ => clean.push(' '),
        }
    }
    let mut words: Vec<&str> = clean.split_whitespace().collect();
    if words.len() > 2 && words[0] == "tom" && words[1] == "clancys" {
        words.drain(..2);
    }
    // "... Deluxe Edition", "... Game of the Year Edition", "... GOTY". Not a bare "Legacy"
    // or "Enhanced": "Hogwarts Legacy" is a game of its own.
    loop {
        let n = words.len();
        if n > 1 && words[n - 1] == "edition" {
            words.pop();
            while words.len() > 1 && EDITION_WORDS.contains(words.last().unwrap()) {
                words.pop();
            }
            if words.len() > 3 && words[words.len() - 3..] == ["game", "of", "the"] {
                words.truncate(words.len() - 3);
            }
        } else if n > 1 && words[n - 1] == "goty" {
            words.pop();
        } else {
            break;
        }
    }
    words.join(" ")
}

/// Two names for one game: equal keys (`name_key`), or the same `canonical` name.
/// "Tom Clancys Rainbow Six Siege" (Medal's folder) and "Rainbow Six Siege" are one game.
pub fn same_game(a: &str, b: &str) -> bool {
    let (ka, kb) = (name_key(a), name_key(b));
    if ka.is_empty() || kb.is_empty() {
        return false;
    }
    ka == kb || name_key(&canonical(a)) == name_key(&canonical(b))
}

/// The game an executable belongs to, from the table of known executables (`stem` lower case, no ".exe").
pub fn known_exe_name(stem: &str) -> Option<&'static str> {
    KNOWN.iter().find(|(exe, _)| *exe == stem).map(|(_, name)| *name)
}

/// Window titles and install folders that give Minecraft away when it runs inside Java.
const MC_TITLES: &[&str] = &["minecraft", "lunar client", "badlion", "feather client", "labymod", "forge", "fabric", "optifine", "hypixel"];
const MC_PATHS: &[&str] = &[
    "\\.minecraft\\", "\\minecraft\\", "\\minecraft launcher\\", "\\.lunarclient\\", "\\lunarclient\\", "\\badlion client\\", "\\.feather\\", "\\feather\\",
    "\\prismlauncher\\", "\\multimc\\", "\\curseforge\\", "\\modrinthapp\\", "\\com.modrinth.theseus\\", "\\gdlauncher", "\\atlauncher\\", "\\.tlauncher\\",
    "\\microsoft.4297127d64ec6_", "\\labymod",
];

/// A program other than the game itself whose icon stands for Minecraft: the Java
/// runtime it runs in only has Java's coffee cup.
fn minecraft_icon_source() -> Option<String> {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let (pf86, pf, local) = (env("ProgramFiles(x86)"), env("ProgramFiles"), env("LOCALAPPDATA"));
    [
        format!(r"{pf86}\Minecraft Launcher\MinecraftLauncher.exe"),
        format!(r"{pf}\Minecraft Launcher\MinecraftLauncher.exe"),
        r"C:\XboxGames\Minecraft Launcher\Content\Minecraft.exe".to_string(),
        format!(r"{local}\Programs\lunarclient\Lunar Client.exe"),
        format!(r"{local}\Programs\Badlion Client\Badlion Client.exe"),
        format!(r"{local}\Programs\feather\Feather Launcher.exe"),
        format!(r"{local}\Programs\PrismLauncher\prismlauncher.exe"),
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file())
}

/// Folders that game stores install into (lower case, with surrounding backslashes).
const GAME_DIRS: &[&str] = &[
    "\\steamapps\\common\\", "\\epic games\\", "\\riot games\\", "\\xboxgames\\", "\\ubisoft game launcher\\games\\", "\\origin games\\",
    "\\ea games\\", "\\gog galaxy\\games\\", "\\gog games\\", "\\roblox\\versions\\", "\\games\\",
];

/// Version-info names that describe an engine or a launcher, not the game.
const GENERIC_NAMES: &[&str] = &["unreal engine", "unity", "java", "openjdk", "bootstrap", "launcher", "electron", "game", "application"];

/// If the window is a game, its name.
pub fn identify(fg: &Foreground) -> Option<String> {
    let lower = fg.exe.to_lowercase();
    let stem = Path::new(&lower).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.is_empty() || NOT_GAMES.contains(&stem.as_str()) || NOT_GAMES_MORE.contains(&stem.as_str()) || crate::gamedetect::denied(&stem).is_some() {
        return None;
    }
    // Minecraft (Java) runs inside javaw.exe, which is also every other Java program.
    // Any one of these gives it away: its window title (the game's or a client's), the
    // window class its graphics library creates, the folder the Java runtime sits in
    // (a launcher's own), or simply a Java window that fills the screen.
    if stem == "javaw" || stem == "java" {
        let title = fg.title.to_lowercase();
        let class = fg.class.to_lowercase();
        let is_mc = MC_TITLES.iter().any(|k| title.contains(k)) || class == "glfw30" || class == "lwjgl" || MC_PATHS.iter().any(|p| lower.contains(p)) || fg.fullscreen;
        return is_mc.then(|| "Minecraft".to_string());
    }
    let known = KNOWN.iter().find(|(exe, _)| *exe == stem).map(|(_, name)| name.to_string());
    let in_library = GAME_DIRS.iter().any(|d| lower.contains(d));
    if known.is_none() && !in_library && !fg.fullscreen {
        return None;
    }
    // One name per game, and a name that means "not a game" (a VPN's window filling the screen) is none.
    let name = canonical(&known.unwrap_or_else(|| display_name(&fg.exe)));
    (name != DESKTOP).then_some(name)
}

/// Parts of a program's name that mark a helper beside a game, not the game itself.
const HELPER_WORDS: &[&str] = &["helper", "crash", "launcher", "updater", "service", "overlay", "installer", "setup", "redist", "anticheat", "easyanticheat", "battleye", "report"];

/// A game among these running programs (full paths), if there is one: a game from the table, a
/// game Glint has seen being played (`seen`: its program is remembered), Minecraft's Java, or any
/// program inside a game store's folder that is not a launcher or helper. It errs on the side of
/// "yes": this decides whether Glint may restart itself for an update.
pub fn game_among(paths: &[String], seen: &[Game]) -> Option<String> {
    for p in paths {
        let lower = p.to_lowercase();
        let stem = Path::new(&lower).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if stem.is_empty() || NOT_GAMES.contains(&stem.as_str()) || NOT_GAMES_MORE.contains(&stem.as_str()) {
            continue;
        }
        if stem == "javaw" || stem == "java" {
            if MC_PATHS.iter().any(|d| lower.contains(d)) {
                return Some("Minecraft".into());
            }
            continue;
        }
        if let Some((_, name)) = KNOWN.iter().find(|(exe, _)| *exe == stem) {
            return Some(name.to_string());
        }
        if let Some(g) = seen.iter().find(|g| !g.exe.is_empty() && g.name != DESKTOP && g.exe.to_lowercase() == lower) {
            return Some(g.name.clone());
        }
        if GAME_DIRS.iter().any(|d| lower.contains(d)) && !lower.contains("\\launcher\\") && !HELPER_WORDS.iter().any(|w| stem.contains(w)) {
            return Some(Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(stem));
        }
    }
    None
}

/// Best name for a game executable that is not in the table: the Steam folder
/// it sits in, else its version info, else the file name.
fn display_name(exe: &str) -> String {
    let lower = exe.to_lowercase();
    if let Some(at) = lower.find("\\steamapps\\common\\") {
        let rest = &exe[at + "\\steamapps\\common\\".len()..];
        if let Some(folder) = rest.split('\\').next().filter(|f| !f.is_empty() && rest.contains('\\')) {
            return folder.replace('_', " ");
        }
    }
    if let Some(name) = win::exe_product(exe) {
        let l = name.to_lowercase();
        if !GENERIC_NAMES.iter().any(|g| l == *g || l.starts_with(&format!("{g} "))) {
            return name;
        }
    }
    Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| DESKTOP.to_string())
}

/// The game in the foreground right now, with its process id and executable.
pub fn detect() -> Option<(String, u32, String)> {
    let fg = win::foreground()?;
    identify(&fg).map(|name| (name, fg.pid, fg.exe))
}

// ---------------------------------------------------------------------------
// Game mode (0.1.27): all visible windows are scored, not only the foreground one.

/// An executable's file name in lower case, the key the user's marks are kept by: "cs2.exe".
pub fn exe_key(path: &str) -> String {
    Path::new(path).file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// What the user told Glint from the GAME / DESKTOP switch's menu, plus what it saw before.
pub struct Marks<'a> {
    /// "Detect the game automatically". Off: only the program picked by hand counts.
    pub auto: bool,
    /// The program picked by hand (`exe_key`), "" = none.
    pub pick: &'a str,
    /// Programs the user called a game / not a game (`exe_key`).
    pub games: &'a [String],
    pub not_games: &'a [String],
    /// Games seen before (`games.json`): their program counts as a game again.
    pub seen: &'a [Game],
    /// The process of the game being recorded now, if any.
    pub current: Option<u32>,
}

impl Marks<'_> {
    /// Nothing marked: game detection as before 0.1.27.
    pub fn none() -> Marks<'static> {
        Marks { auto: true, pick: "", games: &[], not_games: &[], seen: &[], current: None }
    }
}

/// Points a window gets for being a game (None = it is not one), before the bonuses of `choose`.
pub const SCORE_PICK: i64 = 1000;
pub const SCORE_MARKED: i64 = 500;
pub const SCORE_KNOWN: i64 = 300;
pub const SCORE_SEEN: i64 = 200;
pub const SCORE_STORE: i64 = 150;
pub const SCORE_FULLSCREEN: i64 = 80;

/// Windows smaller than this are splash screens, pop-ups and tray helpers, not a game to record
/// (a hand-picked program is taken whatever its size).
const MIN_W: u32 = 320;
const MIN_H: u32 = 200;

fn stem_of(lower_path: &str) -> String {
    Path::new(lower_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn is_minecraft(w: &win::WinInfo) -> bool {
    let lower = w.exe.to_lowercase();
    let title = w.title.to_lowercase();
    let class = w.class.to_lowercase();
    MC_TITLES.iter().any(|k| title.contains(k)) || class == "glfw30" || class == "lwjgl" || MC_PATHS.iter().any(|p| lower.contains(p)) || w.fullscreen
}

/// How much `w` looks like a game. The order: the user's own marks, then the table of known
/// games, games seen before, a game store's folder, and last a window that fills its monitor.
/// Browsers, launchers, players and tools (NOT_GAMES) are never games unless the user says so.
pub fn rate(w: &win::WinInfo, m: &Marks) -> Option<i64> {
    let key = exe_key(&w.exe);
    let lower = w.exe.to_lowercase();
    let stem = stem_of(&lower);
    if key.is_empty() || w.cloaked || m.not_games.iter().any(|n| *n == key) {
        return None;
    }
    if !m.pick.is_empty() && m.pick == key {
        return Some(SCORE_PICK);
    }
    if !m.auto {
        return None;
    }
    if !w.minimized && (w.width < MIN_W || w.height < MIN_H) {
        return None;
    }
    if m.games.iter().any(|g| *g == key) {
        return Some(SCORE_MARKED);
    }
    if NOT_GAMES.contains(&stem.as_str()) || NOT_GAMES_MORE.contains(&stem.as_str()) || crate::gamedetect::denied(&stem).is_some() {
        return None;
    }
    if stem == "javaw" || stem == "java" {
        return is_minecraft(w).then_some(SCORE_KNOWN);
    }
    if KNOWN.iter().any(|(exe, _)| *exe == stem) {
        return Some(SCORE_KNOWN);
    }
    if m.seen.iter().any(|g| !g.exe.is_empty() && g.name != DESKTOP && g.exe.to_lowercase() == lower) {
        return Some(SCORE_SEEN);
    }
    // Store folders and fullscreen windows hold helpers too: a crash reporter, an anti-cheat, a launcher.
    if lower.contains("\\launcher\\") || HELPER_WORDS.iter().any(|h| stem.contains(h)) {
        return None;
    }
    if GAME_DIRS.iter().any(|d| lower.contains(d)) {
        return Some(SCORE_STORE);
    }
    w.fullscreen.then_some(SCORE_FULLSCREEN)
}

/// The window to record among `wins`: the best `rate`, plus 200 for the foreground window, 100 for
/// the game being recorded now (it is kept while it lives, unless another game is played in front
/// of it), 100 less when minimized, and the larger window of one program before its smaller ones.
pub fn choose(wins: &[win::WinInfo], m: &Marks) -> Option<usize> {
    let mut best: Option<(usize, i64, u64)> = None;
    for (i, w) in wins.iter().enumerate() {
        let Some(base) = rate(w, m) else { continue };
        let score = base + if w.foreground { 200 } else { 0 } + if m.current == Some(w.pid) { 100 } else { 0 } - if w.minimized { 100 } else { 0 };
        let area = w.width as u64 * w.height as u64;
        if best.map_or(true, |(_, s, a)| score > s || (score == s && area > a)) {
            best = Some((i, score, area));
        }
    }
    best.map(|(i, _, _)| i)
}

/// The name a window's game is filed under: the table, Minecraft's Java, a game seen before
/// from this program, else its Steam folder or version info; never "Desktop" (a program the
/// user picked whose name means "not a game" keeps its file name).
pub fn name_for(w: &win::WinInfo, seen: &[Game]) -> String {
    let lower = w.exe.to_lowercase();
    let stem = stem_of(&lower);
    if let Some((_, name)) = KNOWN.iter().find(|(exe, _)| *exe == stem) {
        return name.to_string();
    }
    if (stem == "javaw" || stem == "java") && is_minecraft(w) {
        return "Minecraft".into();
    }
    if let Some(g) = seen.iter().find(|g| !g.exe.is_empty() && g.name != DESKTOP && g.exe.to_lowercase() == lower) {
        return g.name.clone();
    }
    let name = canonical(&display_name(&w.exe));
    if name == DESKTOP {
        Path::new(&w.exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Game".into())
    } else {
        name
    }
}

/// `detect` with the user's marks (desktop mode): a program marked "not a game" is none, one
/// picked or marked as a game is one. With nothing marked it is exactly `detect`.
pub fn detect_marked(m: &Marks) -> Option<(String, u32, String)> {
    let fg = win::foreground()?;
    let key = exe_key(&fg.exe);
    if m.not_games.iter().any(|n| *n == key) {
        return None;
    }
    let picked = !m.pick.is_empty() && m.pick == key;
    if picked || (m.auto && m.games.iter().any(|g| *g == key)) {
        let w = win::WinInfo { exe: fg.exe.clone(), title: fg.title.clone(), class: fg.class.clone(), fullscreen: fg.fullscreen, ..Default::default() };
        return Some((name_for(&w, m.seen), fg.pid, fg.exe));
    }
    if !m.auto {
        return None; // detection off: the hand pick only
    }
    identify(&fg).map(|name| (name, fg.pid, fg.exe))
}

pub fn icons_dir() -> PathBuf {
    let d = util::data_dir().join("icons");
    let _ = fs::create_dir_all(&d);
    d
}

/// Every game Glint has seen, saved in `games.json`.
pub struct Games {
    pub list: Vec<Game>,
}

impl Games {
    fn file() -> PathBuf {
        util::data_dir().join("games.json")
    }

    pub fn load() -> Self {
        Games { list: fs::read_to_string(Self::file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default() }
    }

    fn save(&self) {
        if let Ok(t) = serde_json::to_string_pretty(&self.list) {
            let _ = fs::write(Self::file(), t);
        }
    }

    /// Record that `name` runs from `exe`, and take the exe's icon if the game
    /// has none yet. Returns true if anything changed.
    pub fn note(&mut self, name: &str, exe: &str) -> bool {
        // A game seen for the first time is a change in itself, even with no exe (an imported clip's game).
        let mut changed = false;
        let at = match self.list.iter().position(|g| g.name.eq_ignore_ascii_case(name)) {
            Some(i) => i,
            None => {
                self.list.push(Game { name: name.to_string(), ..Default::default() });
                changed = true;
                self.list.len() - 1
            }
        };
        let g = &mut self.list[at];
        if !exe.is_empty() && g.exe != exe {
            g.exe = exe.to_string();
            changed = true;
        }
        // Minecraft's own program is the Java runtime (or unknown, for an imported clip):
        // its icon comes from a launcher on this PC, if there is one.
        let java = Path::new(&exe.to_lowercase()).file_stem().is_some_and(|s| s == "javaw" || s == "java");
        let icon_exe = if g.name == "Minecraft" && (exe.is_empty() || java) { minecraft_icon_source().unwrap_or_default() } else { exe.to_string() };
        if !g.custom_icon && !icon_exe.is_empty() && (g.icon.is_empty() || !Path::new(&g.icon).is_file()) {
            if let Some(icon) = save_exe_icon(name, &icon_exe) {
                g.icon = icon;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
        changed
    }

    /// Drop games that are filed under another name now (see `canonical`) or that no clip
    /// uses any more, keeping ones seen running (they have an exe) and ones with an icon
    /// the user picked. `used` = the game names the library's clips carry.
    pub fn tidy(&mut self, used: &[String]) -> bool {
        let before = self.list.len();
        self.list.retain(|g| canonical(&g.name) == g.name && (g.custom_icon || !g.exe.is_empty() || used.iter().any(|u| u.eq_ignore_ascii_case(&g.name))));
        let changed = self.list.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    /// Games that have no usable icon and no icon the user picked: the ones to look one up for.
    pub fn missing_icons(&self) -> Vec<Game> {
        self.list.iter().filter(|g| !g.custom_icon && g.name != DESKTOP && g.name != "Imported" && (g.icon.is_empty() || !Path::new(&g.icon).is_file())).cloned().collect()
    }

    /// Give `name` an icon found on this PC (`icons.rs`). `again`: the user asked for it
    /// ("Find icon again"), so it replaces any icon, a picked one too; otherwise only a
    /// missing icon is filled and a picked one is never touched. True if it was set.
    pub fn set_found_icon(&mut self, name: &str, icon: &str, again: bool) -> bool {
        let Some(g) = self.list.iter_mut().find(|g| g.name.eq_ignore_ascii_case(name)) else { return false };
        if !again && (g.custom_icon || (!g.icon.is_empty() && Path::new(&g.icon).is_file())) {
            return false;
        }
        // The file it replaces goes, if it is one of ours.
        if !g.icon.is_empty() && g.icon != icon && Path::new(&g.icon).starts_with(icons_dir()) {
            let _ = fs::remove_file(&g.icon);
        }
        g.icon = icon.to_string();
        g.custom_icon = false;
        self.save();
        true
    }

    /// Use an image the user picked as the game's icon.
    pub fn set_icon(&mut self, name: &str, image: &Path) -> Result<Game, String> {
        let ext = image.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !["png", "jpg", "jpeg", "webp", "ico", "bmp", "gif"].contains(&ext.as_str()) {
            return Err("Pick a PNG, JPG, WEBP, ICO, BMP or GIF image".into());
        }
        let to = icons_dir().join(format!("{}-custom-{}.{ext}", util::sanitize_filename(name), util::new_id()));
        fs::copy(image, &to).map_err(|e| format!("Could not copy the image: {e}"))?;
        let at = match self.list.iter().position(|g| g.name.eq_ignore_ascii_case(name)) {
            Some(i) => i,
            None => {
                self.list.push(Game { name: name.to_string(), ..Default::default() });
                self.list.len() - 1
            }
        };
        let g = &mut self.list[at];
        if g.custom_icon {
            let _ = fs::remove_file(&g.icon);
        }
        g.icon = to.to_string_lossy().into_owned();
        g.custom_icon = true;
        let out = g.clone();
        self.save();
        Ok(out)
    }
}

/// Write the executable's icon as `icons/<game>.png`. Returns the file path.
fn save_exe_icon(name: &str, exe: &str) -> Option<String> {
    const SIZE: u32 = 64;
    let rgba = win::exe_icon_rgba(exe, SIZE)?;
    let path = icons_dir().join(format!("{}.png", util::sanitize_filename(name)));
    let file = fs::File::create(&path).ok()?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), SIZE, SIZE);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(&rgba).ok()?;
    Some(path.to_string_lossy().into_owned())
}
