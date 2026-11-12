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
    /// "clip" | "toggle" | "show"
    pub action: String,
    /// Clip length for the "clip" action.
    pub seconds: u32,
}

impl Default for Keybind {
    fn default() -> Self {
        Keybind { id: util::new_id(), ctrl: false, shift: false, alt: false, win: false, key: String::new(), action: "clip".into(), seconds: 30 }
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
            gpu_encoding: false,
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

/// A named colour correction the user saved in the editor. 1.0 = unchanged.
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub clips_dir: String,
    /// Save each clip into a subfolder named after its game: `<clips_dir>\Overwatch\…`.
    pub game_folders: bool,
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
    pub record_on_start: bool,
    pub sound: bool,
    /// One of `sound::SOUNDS`, or "custom" to play `sound_file`.
    pub sound_name: String,
    /// 0..=100. Not applied to a custom file.
    pub sound_volume: u32,
    pub sound_file: String,
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
    /// Colour theme id (see ui/themes.js) and the user's own CSS, applied after it.
    pub theme: String,
    pub custom_css: String,
    /// Discord Rich Presence, and the Application ID it identifies as.
    pub discord_rpc: bool,
    pub discord_app_id: String,
    /// Ask GitHub once a day whether a newer Glint has been released.
    pub update_check: bool,
    /// Interface language: "en", "ko" or "el".
    pub language: String,
    /// How many Blur renders may run at the same time; more wait in a queue.
    pub blur_parallel: u32,
    /// Size of the whole interface, in percent.
    pub ui_zoom: u32,
    /// The user's own colour-correction presets for the editor.
    pub color_presets: Vec<ColorPreset>,
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
            record_on_start: true,
            sound: true,
            sound_name: "chirp".into(),
            sound_volume: 40,
            sound_file: String::new(),
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
            discord_rpc: false,
            discord_app_id: String::new(),
            update_check: true,
            language: "en".into(),
            blur_parallel: 2,
            ui_zoom: 100,
            color_presets: Vec::new(),
            autostart: false,
            close_to_tray: true,
            ffmpeg_path: String::new(),
            blur_path: String::new(),
            keybinds: vec![
                Keybind { id: "kb-30s".into(), key: "F8".into(), seconds: 30, ..Default::default() },
                Keybind { id: "kb-10m".into(), key: "F8".into(), shift: true, seconds: 600, ..Default::default() },
            ],
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

    pub fn default_clip_secs(&self) -> u32 {
        self.keybinds
            .iter()
            .filter(|k| k.action == "clip")
            .map(|k| k.seconds)
            .min()
            .unwrap_or(30)
    }
}
