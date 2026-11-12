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
    if stem.is_empty() || NOT_GAMES.contains(&stem.as_str()) {
        return None;
    }
    // Minecraft (Java) runs inside javaw.exe: only its window title tells it apart.
    if stem == "javaw" || stem == "java" {
        let t = fg.title.to_lowercase();
        return ["minecraft", "lunar client", "badlion", "feather client"].iter().any(|k| t.contains(k)).then(|| "Minecraft".to_string());
    }
    let known = KNOWN.iter().find(|(exe, _)| *exe == stem).map(|(_, name)| name.to_string());
    let in_library = GAME_DIRS.iter().any(|d| lower.contains(d));
    if known.is_none() && !in_library && !fg.fullscreen {
        return None;
    }
    Some(known.unwrap_or_else(|| display_name(&fg.exe)))
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
        if !g.custom_icon && !exe.is_empty() && (g.icon.is_empty() || !Path::new(&g.icon).is_file()) {
            if let Some(icon) = save_exe_icon(name, exe) {
                g.icon = icon;
                changed = true;
            }
        }
        if changed {
            self.save();
        }
        changed
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
