use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Keybind {
    pub id: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
    /// KeyboardEvent.code of the main key, e.g. "F8", "KeyG", "Numpad5".
    pub key: String,
    /// "clip" | "toggle" | "show" | "edit" (open the newest clip in the editor) | "monitor" (switch the capture monitor: to `monitor`, or to the next one when that is -1)
    pub action: String,
    /// Clip length for the "clip" action.
    pub seconds: u32,
    /// Monitor the "clip" action saves: -1 = the capture monitor (`Settings::monitor`),
    /// 0.. = an index into `win::monitors()`. An index that no longer exists counts as -1.
    pub monitor: i32,
}

impl Default for Keybind {
    fn default() -> Self {
        Keybind { id: util::new_id(), ctrl: false, shift: false, alt: false, win: false, key: String::new(), action: "clip".into(), seconds: 30, monitor: -1 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
/// One field per setting in blur v2's `.blur-config.cfg`, in the file's own
/// order, with blur's defaults. Left out: `preview` (Glint has no preview
/// window), `detailed filenames` (Glint names the output) and the GUI-only keys.
pub struct BlurSettings {
    // - blur
    pub blur: bool,
    pub amount: f32,
    pub output_fps: u32,
    /// equal | gaussian_sym | vegas | pyramid | gaussian | ascending | descending
    /// | gaussian_reverse, or custom weights such as "1, 2, 3, 2, 1".
    pub weighting: String,
    pub gamma: f32,
    // - interpolation
    pub interpolate: bool,
    /// A multiplier ("5x") or a frame rate ("480").
    pub interpolated_fps: String,
    /// "svp" | "rife"
    pub interpolation_method: String,
    // - pre-interpolation
    pub pre_interpolate: bool,
    pub pre_interpolated_fps: String,
    // - deduplication
    pub deduplicate: bool,
    /// "svp" | "rife" | "old"
    pub deduplicate_method: String,
    // - rendering
    /// "h264" | "h265" | "av1" | "vp9"
    pub encode_preset: String,
    pub quality: u32,
    pub copy_dates: bool,
    // - gpu acceleration
    pub gpu_decoding: bool,
    pub gpu_interpolation: bool,
    pub gpu_encoding: bool,
    /// "nvidia" | "amd" | "intel"
    pub gpu_type: String,
    /// -1 = let blur benchmark and choose.
    pub rife_gpu: i32,
    // - timescale
    pub timescale: bool,
    pub input_timescale: f32,
    pub output_timescale: f32,
    pub adjust_audio_pitch: bool,
    // - filters
    pub filters: bool,
    pub brightness: f32,
    pub saturation: f32,
    pub contrast: f32,
    // - advanced (the fields below are written only when this is on)
    pub advanced: bool,
    /// -1 = unlimited.
    pub deduplicate_range: i32,
    pub deduplicate_threshold: f64,
    pub container: String,
    pub ffmpeg_filters: String,
    pub debug: bool,
    pub gaussian_std_dev: f32,
    pub gaussian_mean: f32,
    pub gaussian_bound: String,
    pub svp_preset: String,
    pub svp_algorithm: i32,
    pub block_size: i32,
    pub mask_area: i32,
    pub rife_model: String,
}

impl Default for BlurSettings {
    fn default() -> Self {
        BlurSettings {
            blur: true,
            amount: 1.0,
            output_fps: 60,
            weighting: "equal".into(),
            gamma: 1.0,
            interpolate: true,
            interpolated_fps: "5x".into(),
            interpolation_method: "svp".into(),
            pre_interpolate: false,
            pre_interpolated_fps: "360".into(),
            deduplicate: true,
            deduplicate_method: "svp".into(),
            encode_preset: "h264".into(),
            quality: 16,
            copy_dates: false,
            gpu_decoding: true,
            gpu_interpolation: true,
            // On for new installs (0.1.20); blur::for_this_pc fits gpu_type to the PC's card, and turns it off without one.
            gpu_encoding: true,
            gpu_type: "nvidia".into(),
            rife_gpu: -1,
            timescale: false,
            input_timescale: 1.0,
            output_timescale: 1.0,
            adjust_audio_pitch: false,
            filters: false,
            brightness: 1.0,
            saturation: 1.0,
            contrast: 1.0,
            advanced: false,
            deduplicate_range: 2,
            deduplicate_threshold: 0.001,
            container: "mp4".into(),
            ffmpeg_filters: String::new(),
            debug: false,
            gaussian_std_dev: 1.0,
            gaussian_mean: 2.0,
            gaussian_bound: "[0,2]".into(),
            svp_preset: "weak".into(),
            svp_algorithm: 13,
            block_size: 8,
            mask_area: 0,
            rife_model: "rife-v4.26_ensembleFalse".into(),
        }
    }
}

/// A named color correction the user saved in the editor. 1.0 = unchanged.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ColorPreset {
    pub name: String,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
}

impl Default for ColorPreset {
    fn default() -> Self {
        ColorPreset { name: String::new(), brightness: 1.0, contrast: 1.0, saturation: 1.0, gamma: 1.0 }
    }
}

/// A theme preset the user made (Settings → Appearance): a built-in theme (`base`, an id
/// in ui/themes.js) with the user's own colors (variable -> "#rrggbb") and CSS on top.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ThemePreset {
    pub id: String,
    pub name: String,
    pub base: String,
    pub colors: std::collections::BTreeMap<String, String>,
    pub css: String,
}

impl Default for ThemePreset {
    fn default() -> Self {
        ThemePreset { id: String::new(), name: String::new(), base: "glint".into(), colors: Default::default(), css: String::new() }
    }
}

/// A canvas layout the user saved in the editor (0.1.26): an output size,
/// a background and the pieces of the picture placed on it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct CanvasPreset {
    pub id: String,
    pub name: String,
    pub canvas: crate::editor::Canvas,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub clips_dir: String,
    /// Save each clip into a subfolder named after its game: `<clips_dir>\Overwatch\…`.
    pub game_folders: bool,
    /// How a new clip is named: blocks in order (`naming::NameBlock`: game, date, time, length,
    /// text). Default Game Date Time = "Overwatch 2026-11-14 13-35-44"; empty = the default.
    pub clip_name: Vec<crate::naming::NameBlock>,
    pub monitor: u32,
    /// Output height in pixels. 0 = native.
    pub height: u32,
    /// Shape of the clip. "auto" = widen a stretched resolution back to the
    /// display's own shape; "source" = keep what was captured; or "16:9" etc.
    pub aspect: String,
    pub fps: u32,
    /// "low" | "medium" | "high" | "ultra"
    pub quality: String,
    /// Fixed video bitrate in Mbps. 0 = automatic (quality-targeted, see `quality`).
    pub bitrate: u32,
    /// "auto" | "nvenc" | "amf" | "qsv" | "x264"
    pub encoder: String,
    /// "auto" | "wgc" | "dda"
    pub method: String,
    /// Where the replay buffer lives: "mixed" (the newest `buffer_ram_secs`
    /// seconds in RAM, everything older in files), "ram" or "disk".
    pub buffer: String,
    pub buffer_ram_secs: u32,
    /// Folder for the buffer files. "" = `%LOCALAPPDATA%\Glint\buffer`.
    pub buffer_dir: String,
    pub cursor: bool,
    pub desktop_audio: bool,
    pub mic: bool,
    /// Clean the microphone track with RNNoise when a clip is saved.
    pub mic_denoise: bool,
    /// Also record the current game's own sound (its process tree) as an extra "Game" track.
    pub game_audio: bool,
    /// The mixer's faders in Settings → Audio, in dB (0 = as captured): applied
    /// as the sound is captured, so clips and meters hold the result.
    pub desktop_gain_db: f32,
    pub mic_gain_db: f32,
    pub game_gain_db: f32,
    /// Settings → Audio: the output device whose sound is recorded and the microphone,
    /// by name. "" = the Windows default; a device that is not there falls back to it.
    pub desktop_device: String,
    pub mic_device: String,
    /// The top bar's GAME / DESKTOP switch (0.1.27, capmode.rs): "desktop" records the whole
    /// monitor and all desktop sound (as always), "game" only the game's window and its own sound.
    pub capture_mode: String,
    /// Look for the game by itself; off = only the program picked by hand (`game_pick`).
    pub game_auto: bool,
    /// Marks from the switch's right-click menu, by executable name in lower case ("cs2.exe"):
    /// the program picked by hand ("" = none), programs that are games, programs that are not.
    pub game_pick: String,
    pub game_exes: Vec<String>,
    pub not_game_exes: Vec<String>,
    pub record_on_start: bool,
    pub sound: bool,
    /// One of `sound::SOUNDS`, or "custom" to play `sound_file`.
    pub sound_name: String,
    /// 0..=100. Not applied to a custom file.
    pub sound_volume: u32,
    pub sound_file: String,
    /// The part of a custom sound file that is played: from sound_start seconds
    /// to sound_end (0 = to the file's end).
    pub sound_start: f32,
    pub sound_end: f32,
    pub toast: bool,
    /// Corner of the recorded monitor for the clip overlay: "tl" | "tr" | "bl" | "br".
    pub toast_pos: String,
    /// Overlay opacity, 20..=100 percent.
    pub toast_opacity: u32,
    /// Overlay size, 70..=160 percent.
    pub toast_scale: u32,
    /// Seconds the overlay stays up.
    pub toast_secs: f32,
    /// Show a saved clip as just the Glint mark in a small square, with no text.
    pub toast_compact: bool,
    /// Parts of the overlay text "Captured last 30s of Overwatch", and the microphone icon.
    pub toast_length: bool,
    pub toast_game: bool,
    pub toast_mic: bool,
    /// Color theme id (see ui/themes.js) and the user's own CSS, applied after it.
    pub theme: String,
    pub custom_css: String,
    /// Color the taskbar and tray icon with the theme's accent. The UI stores the
    /// accent colors it computed (`--accent`, `--accent-2`, "#rrggbb") so the
    /// icon is right at the next start before the window has loaded.
    pub icon_theme: bool,
    pub icon_accent: String,
    pub icon_accent_2: String,
    /// The user's own colors from Settings → Appearance → Custom colors: theme
    /// variable name ("accent", "bg", ...) -> "#rrggbb". Applied on top of the
    /// theme and under `custom_css` (ui/themes.js), in the window and the overlay.
    pub theme_colors: std::collections::BTreeMap<String, String>,
    /// The user's own theme presets (Settings → Appearance, 0.1.20) and the id of the
    /// active one ("" = a built-in theme). `theme`, `theme_colors` and `custom_css`
    /// above always hold what the active preset has (empty for a built-in theme), so
    /// the overlay and the icon read only those; ui/themes.js keeps them in step.
    pub theme_presets: Vec<ThemePreset>,
    /// The order of the theme tiles under Settings → Appearance, when the user dragged them: built-in theme
    /// ids and "p:<preset id>". Tiles not listed follow in their usual order (colorpick.js prTileOrder).
    pub theme_order: Vec<String>,
    pub theme_preset: String,
    /// Library folder colors: folder name -> "#rrggbb" (right-click a folder → Color).
    /// Renaming a folder carries its color along, deleting it drops it (`carry_folder_color`).
    pub folder_colors: std::collections::BTreeMap<String, String>,
    /// Which kind tags clip cards show ("import", "trim", "edit", "blur", "discord", ...). Empty = none
    /// (0.1.21 removed the tags; 0.1.24 brought them back as a choice).
    pub card_tags: Vec<String>,
    /// How visible the previous / next clip arrows are in a fullscreen clip, in percent (20..=100).
    pub nav_opacity: u32,
    /// Ask GitHub once a day whether a newer Glint has been released.
    pub update_check: bool,
    /// Install a waiting update without being asked, while no game is running, Glint's
    /// window is closed and nothing is being rendered, exported or imported.
    pub update_auto: bool,
    /// Interface language: "en", "ko" or "el".
    pub language: String,
    /// How many Blur renders may run at the same time; more wait in a queue.
    pub blur_parallel: u32,
    /// Size of the whole interface, in percent.
    pub ui_zoom: u32,
    /// When the top bar's renders button shows: "auto" (while something is
    /// rendering), "always" or "never".
    pub renders_button: String,
    /// The user's own color-correction presets for the editor.
    pub color_presets: Vec<ColorPreset>,
    /// Show the library's 'Main folder' chip even when no clip sits loose in the clips folder.
    pub show_empty_main: bool,
    /// Editor shortcuts the user changed: command -> key combinations such as
    /// "Ctrl+Shift+KeyZ" (see `ED_KEYS` in ui/editor.js). A command that is
    /// not listed keeps its default keys; an empty list means no shortcut.
    pub editor_keys: std::collections::BTreeMap<String, Vec<String>>,
    /// The default size for "Compress for Discord", in MB (1 MB = 1,000,000
    /// bytes), set on the Compress page: the menu lists it first. 5..=1000.
    /// (Before 0.1.16 it was the size picked last.)
    pub compress_mb: u32,
    /// The Compress page's settings (sizes in the menu, preset, audio, caps, where the result goes).
    pub compress: crate::compress::CompressSettings,
    /// The last choices of the "Export as GIF" dialog (gif.rs).
    pub gif: crate::gif::GifOpts,
    /// The last choices of the "Upscale" dialog (upscale.rs).
    pub upscale: crate::upscale::UpscaleOpts,
    /// The user's render rules (rules.rs): named chains of steps run on a clip with one click.
    pub rules: Vec<crate::rules::Rule>,
    /// The rule the editor's Export box runs on the exported clip ("Then run"); "" = none.
    pub rules_after_export: String,
    /// The user's saved canvas layouts for the editor's export (vertical videos and other sizes).
    pub canvas_presets: Vec<CanvasPreset>,
    /// Soft sounds for clicks, switches and results in Glint's window.
    pub ui_sounds: bool,
    /// Also a very quiet tick when the pointer moves onto a button (needs `ui_sounds`).
    pub ui_sounds_hover: bool,
    /// Volume of the interface sounds, in percent (0..=100).
    pub ui_sounds_volume: u32,
    pub autostart: bool,
    pub close_to_tray: bool,
    pub ffmpeg_path: String,
    pub blur_path: String,
    pub keybinds: Vec<Keybind>,
    pub blur: BlurSettings,
}

impl Default for Settings {
    fn default() -> Self {
        let videos = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Videos").join("Glint");
        Settings {
            clips_dir: videos.to_string_lossy().into_owned(),
            game_folders: false,
            clip_name: crate::naming::default_blocks(),
            monitor: 0,
            height: 0,
            aspect: "auto".into(),
            fps: 60,
            quality: "high".into(),
            bitrate: 0,
            encoder: "auto".into(),
            method: "auto".into(),
            buffer: "mixed".into(),
            buffer_ram_secs: 30,
            buffer_dir: String::new(),
            cursor: true,
            desktop_audio: true,
            mic: true,
            mic_denoise: false,
            game_audio: false,
            desktop_device: String::new(),
            mic_device: String::new(),
            capture_mode: "desktop".into(),
            game_auto: true,
            game_pick: String::new(),
            game_exes: Vec::new(),
            not_game_exes: Vec::new(),
            record_on_start: true,
            sound: true,
            sound_name: "chirp".into(),
            sound_volume: 40,
            sound_file: String::new(),
            sound_start: 0.0,
            desktop_gain_db: 0.0,
            mic_gain_db: 0.0,
            game_gain_db: 0.0,
            sound_end: 0.0,
            toast: true,
            toast_pos: "tr".into(),
            toast_opacity: 96,
            toast_scale: 100,
            toast_secs: 3.5,
            toast_compact: false,
            toast_length: true,
            toast_game: true,
            toast_mic: true,
            theme: "glint".into(),
            custom_css: String::new(),
            icon_theme: false,
            icon_accent: String::new(),
            icon_accent_2: String::new(),
            theme_colors: Default::default(),
            theme_presets: Vec::new(),
            theme_order: Vec::new(),
            theme_preset: String::new(),
            folder_colors: Default::default(),
            card_tags: Vec::new(),
            nav_opacity: 60,
            update_check: true,
            update_auto: true,
            language: "en".into(),
            blur_parallel: 2,
            ui_zoom: 100,
            renders_button: "auto".into(),
            color_presets: Vec::new(),
            show_empty_main: false,
            editor_keys: Default::default(),
            compress_mb: 10,
            compress: Default::default(),
            gif: Default::default(),
            upscale: Default::default(),
            rules: Vec::new(),
            rules_after_export: String::new(),
            canvas_presets: Vec::new(),
            ui_sounds: false,
            ui_sounds_hover: false,
            ui_sounds_volume: 50,
            autostart: false,
            close_to_tray: true,
            ffmpeg_path: String::new(),
            blur_path: String::new(),
            // A fresh Glint has one keybind (0.1.18: the Shift+F8 ten-minute one is gone).
            // Saved settings keep whatever keybinds they list.
            keybinds: vec![Keybind { id: "kb-30s".into(), key: "F8".into(), seconds: 30, ..Default::default() }],
            blur: BlurSettings::default(),
        }
    }
}

impl Settings {
    fn file() -> PathBuf {
        util::data_dir().join("settings.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = fs::write(Self::file(), t);
        }
    }

    /// Longest clip any keybind can ask for; the buffer is sized to this.
    pub fn max_clip_secs(&self) -> u32 {
        self.keybinds
            .iter()
            .filter(|k| k.action == "clip")
            .map(|k| k.seconds)
            .max()
            .unwrap_or(30)
            .clamp(5, 3600)
    }

    /// A library folder was renamed (`to` = Some) or deleted (None): its color follows it or
    /// goes. Names match ignoring letter case, as folder names do on Windows. True if it had one.
    pub fn carry_folder_color(&mut self, from: &str, to: Option<&str>) -> bool {
        let Some(key) = self.folder_colors.keys().find(|k| k.to_lowercase() == from.to_lowercase()).cloned() else { return false };
        let color = self.folder_colors.remove(&key).unwrap_or_default();
        if let Some(to) = to {
            self.folder_colors.insert(to.to_string(), color);
        }
        true
    }

    pub fn default_clip_secs(&self) -> u32 {
        self.keybinds
            .iter()
            .filter(|k| k.action == "clip")
            .map(|k| k.seconds)
            .min()
            .unwrap_or(30)
    }
}
