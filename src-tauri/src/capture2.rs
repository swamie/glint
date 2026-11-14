//! Capture extras (0.1.16):
//! - Windows' "Screenshot borders" setting, read (never changed) so the UI can
//!   explain the yellow Graphics Capture border and "auto" can avoid it,
//! - what every automatic capture choice resolves to right now (`auto_info`),
//! - "Apply recommended settings": the rules in ONE place (`recommend`), with
//!   the hardware facts they use (`recommended_settings`).
//! Self-test checks run inside `--polish-test`.
use crate::core::{self, Core};
use crate::settings::Settings;
use crate::{blur, recorder, util, win};
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;

/* ---------- the screenshot border setting ---------- */

/// First Windows build with `GraphicsCaptureSession.IsBorderRequired` (Windows 10 2104 / Server 2022).
/// On older builds no program can turn the border off.
pub const BORDER_API_BUILD: u32 = 20348;

/// Windows' consent for "capture without the yellow border", as stored by
/// Settings > Privacy & security > Screenshot borders.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct BorderInfo {
    pub build: u32,
    /// The Windows build has the API to turn the border off at all.
    pub configurable: bool,
    /// The page's main switch ("Screenshot borders" access): "Allow", "Deny" or "" (not set).
    pub main: String,
    /// "Let desktop apps turn off the screenshot border": "Allow", "Deny" or "".
    pub desktop_apps: String,
    /// The device-wide value (an administrator or a policy): "Allow", "Deny" or "".
    pub device: String,
    /// Nothing above says no: a capture that asks for no border should get none.
    pub can_remove: bool,
}

/// Pure decision, so the self-test can feed it made-up registry values.
pub fn border_decide(build: u32, main: &str, desktop_apps: &str, device: &str) -> BorderInfo {
    let configurable = build == 0 || build >= BORDER_API_BUILD;
    let deny = |v: &str| v.eq_ignore_ascii_case("deny");
    BorderInfo {
        build,
        configurable,
        main: main.into(),
        desktop_apps: desktop_apps.into(),
        device: device.into(),
        can_remove: configurable && !deny(main) && !deny(desktop_apps) && !deny(device),
    }
}

/// Read the setting from the registry (read only; Glint never changes it).
pub fn border_info() -> BorderInfo {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\graphicsCaptureWithoutBorder";
    let val = |root, sub: &str| -> String { RegKey::predef(root).open_subkey(sub).and_then(|k| k.get_value::<String, _>("Value")).unwrap_or_default() };
    border_decide(win::windows_build(), &val(HKEY_CURRENT_USER, KEY), &val(HKEY_CURRENT_USER, &format!(r"{KEY}\NonPackaged")), &val(HKEY_LOCAL_MACHINE, KEY))
}

/* ---------- this PC ---------- */

#[repr(C)]
#[allow(non_snake_case)]
struct MemoryStatusEx {
    dwLength: u32,
    dwMemoryLoad: u32,
    ullTotalPhys: u64,
    ullAvailPhys: u64,
    ullTotalPageFile: u64,
    ullAvailPageFile: u64,
    ullTotalVirtual: u64,
    ullAvailVirtual: u64,
    ullAvailExtendedVirtual: u64,
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
}

/// Installed RAM in bytes (0 if unknown).
pub fn ram_bytes() -> u64 {
    let mut m = MemoryStatusEx { dwLength: std::mem::size_of::<MemoryStatusEx>() as u32, dwMemoryLoad: 0, ullTotalPhys: 0, ullAvailPhys: 0, ullTotalPageFile: 0, ullAvailPageFile: 0, ullTotalVirtual: 0, ullAvailVirtual: 0, ullAvailExtendedVirtual: 0 };
    // SAFETY: a correctly sized, writable struct with dwLength set, as the API asks.
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 { m.ullTotalPhys } else { 0 }
}

pub fn cpu_threads() -> u32 {
    std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1)
}

/* ---------- what "Auto" is using ---------- */

/// What each automatic capture choice resolves to now. Names are the setting
/// values ("nvenc", "wgc", ...); the UI turns them into words.
#[derive(Serialize, Clone, Debug, Default)]
pub struct AutoInfo {
    /// Encoder "auto" uses: the running capture's (after any fallback), else ffmpeg's test encode. "" = not known yet.
    pub encoder: String,
    /// Capture method "auto" uses: "wgc" or "dda". "" = no ffmpeg.
    pub method: String,
    /// Why "auto" took Desktop Duplication: "windows10", "border", "no_wgc" or "".
    pub method_why: String,
    /// The clip size "Aspect ratio: Auto" gives at the chosen resolution, the
    /// display mode it is captured from, and whether Auto widened it.
    pub aspect_width: u32,
    pub aspect_height: u32,
    pub mode_width: u32,
    pub mode_height: u32,
    pub widened: bool,
    /// "Bitrate: Auto" ceiling at the chosen quality, Mbps (the capture's own scaling for refresh rates not included).
    pub auto_mbps: f64,
    /// "Native" resolution: the capture monitor's current mode.
    pub native_width: u32,
    pub native_height: u32,
    pub ffmpeg: Option<String>,
    pub blur: Option<String>,
    pub border: BorderInfo,
}

/// Why "auto" picks the method it picks (mirror of `polish::pick_method_border`).
pub fn method_why(has_wgc: bool, build: u32, border_ok: bool) -> &'static str {
    if !has_wgc {
        "no_wgc"
    } else if build != 0 && build < crate::polish::BORDERLESS_WGC_BUILD {
        "windows10"
    } else if !border_ok {
        "border"
    } else {
        ""
    }
}

/// `probe`: run ffmpeg's test encode if the encoder is not known yet (takes a second or two once).
pub fn resolve_auto(core: &Core, probe: bool) -> AutoInfo {
    let s = core.settings();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg");
    let st = core.status();
    let border = border_info();
    let mut a = AutoInfo { border: border.clone(), ffmpeg: ffmpeg.as_ref().map(|p| p.to_string_lossy().into_owned()), blur: blur::find(&s).map(|p| p.to_string_lossy().into_owned()), ..Default::default() };
    if let Some(ff) = &ffmpeg {
        a.encoder = if st.recording && !st.encoder.is_empty() && s.encoder == "auto" {
            st.encoder.clone()
        } else if probe {
            recorder::resolve_encoder(ff, "auto")
        } else {
            recorder::cached_encoder(ff).unwrap_or_default()
        };
        let has_wgc = recorder::has_wgc(ff);
        a.method = recorder::resolve_method(ff, "auto");
        a.method_why = method_why(has_wgc, border.build, border.can_remove).into();
    }
    let mons = win::monitors();
    if !mons.is_empty() {
        let m = &mons[core::capture_monitor(&s, mons.len()) as usize];
        let mut auto = s.clone();
        let (w, h) = core::output_size((m.width, m.height), (m.native_width, m.native_height), s.height, "auto");
        let (sw, _) = core::output_size((m.width, m.height), (m.native_width, m.native_height), s.height, "source");
        a.aspect_width = w;
        a.aspect_height = h;
        a.widened = w != sw;
        a.mode_width = m.width;
        a.mode_height = m.height;
        a.native_width = m.width;
        a.native_height = m.height;
        auto.bitrate = 0;
        a.auto_mbps = (core::capture_cfg_for(&auto, m, 30).maxrate() as f64 / 1e6 * 10.0).round() / 10.0;
    }
    a
}

#[tauri::command(async)]
pub fn auto_info(core: tauri::State<'_, Arc<Core>>) -> AutoInfo {
    resolve_auto(&core, true)
}

/* ---------- recommended settings ---------- */

/// What the rules look at.
#[derive(Serialize, Clone, Debug)]
pub struct RecInput {
    /// The graphics card's name ("" = none found) and the encoder that passed ffmpeg's test encode.
    pub gpu: String,
    pub encoder: String,
    /// The capture monitor's current mode.
    pub width: u32,
    pub height: u32,
    pub hz: u32,
    pub ram_gb: f64,
    pub threads: u32,
    pub build: u32,
    pub has_wgc: bool,
    pub border_ok: bool,
    /// The longest clip a keybind saves, seconds.
    pub longest_secs: u32,
}

/// One setting to change: `key` is the settings field, `value` what it becomes.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RecItem {
    pub key: String,
    pub value: Value,
    /// English, shown translated.
    pub why: String,
    /// Values for the why's {placeholders}.
    pub vars: Value,
}

#[derive(Serialize, Clone, Debug)]
pub struct Recommended {
    pub input: RecInput,
    pub items: Vec<RecItem>,
}

/// Pixels per second the encoder handles comfortably while a game also uses the PC.
fn pixel_budget(encoder: &str, threads: u32) -> f64 {
    match encoder {
        "nvenc" => 600e6,
        "amf" => 400e6,
        "qsv" => 300e6,
        _ => threads.max(1) as f64 * 12e6, // x264 ultrafast, roughly, leaving room for the game
    }
}

/// Resolutions the Capture page offers (below the mode's own height), largest first.
const HEIGHTS: [u32; 7] = [2160, 1600, 1440, 1200, 1080, 900, 720];

/// The rules. Values match what the Capture page's dropdowns offer.
pub fn recommend(i: &RecInput) -> Vec<RecItem> {
    let item = |key: &str, value: Value, why: &str, vars: Value| RecItem { key: key.into(), value, why: why.into(), vars };
    let hw = i.encoder != "x264" && !i.encoder.is_empty();
    let budget = pixel_budget(&i.encoder, i.threads);
    let (w, h, hz) = (i.width.max(1) as f64, i.height.max(1), i.hz.max(1));
    let px = |height: u32| w * height as f64 / h as f64 * height as f64;
    // Frame rate: 120 on a fast screen when the encoder has room for it at full size, else 60 (or the screen's own rate below 60).
    let base = hz.min(60);
    let mut fps = if hw && hz >= 120 && px(h) * 120.0 <= budget { 120 } else { base };
    // Resolution: the screen's own, else the largest step that fits; at 720p and still too much, halve the frame rate.
    let fits = |height: u32, f: u32| px(height) * f as f64 <= budget;
    let mut height = 0u32;
    if !fits(h, fps) {
        height = HEIGHTS.iter().copied().filter(|&x| x < h).find(|&x| fits(x, fps)).unwrap_or(720.min(h));
        if !fits(height, fps) && fps > 30 {
            fps = 30;
            height = if fits(h, 30) { 0 } else { HEIGHTS.iter().copied().filter(|&x| x < h).find(|&x| fits(x, 30)).unwrap_or(720.min(h)) };
        }
        if height >= h {
            height = 0;
        }
    }
    let shown_h = if height == 0 { h } else { height };
    let mut out = vec![
        item("encoder", json!("auto"), if hw { "Auto uses {encoder}, which passed a test encode on this PC" } else { "No hardware encoder passed a test encode, so Auto uses x264 on the processor" }, json!({ "encoder": i.encoder })),
        item(
            "method",
            json!("auto"),
            match method_why(i.has_wgc, i.build, i.border_ok) {
                "no_wgc" => "This ffmpeg has no Graphics Capture, so Auto uses Desktop Duplication",
                "windows10" => "On Windows 10 Auto uses Desktop Duplication, which has no yellow border",
                "border" => "Windows does not let desktop apps hide the capture border, so Auto uses Desktop Duplication",
                _ => "Auto uses Windows Graphics Capture, the smoothest at high frame rates",
            },
            json!({}),
        ),
        item("aspect", json!("auto"), "Fixes stretched resolutions and leaves everything else alone", json!({})),
        item(
            "height",
            json!(height),
            if height == 0 { "Your screen's own size, which {encoder} encodes comfortably at {fps} fps" } else { "{encoder} would struggle with your screen's full size at {fps} fps" },
            json!({ "encoder": i.encoder, "fps": fps }),
        ),
        item("fps", json!(fps), if fps >= 120 { "Your {hz} Hz screen and {encoder} have room for 120 fps, which suits Blur" } else { "Smooth playback at a load {encoder} handles while you play" }, json!({ "hz": hz, "encoder": i.encoder })),
        item("bitrate", json!(0), "Uses only as much as the picture needs, which keeps the buffer small", json!({})),
        item("quality", json!(if hw { "high" } else { "medium" }), if hw { "High looks clean and the graphics card does the work" } else { "Medium keeps the processor's load and the files smaller" }, json!({})),
    ];
    // Buffer: the worst case of the longest clip at the quality ceiling (mirrors CaptureCfg::maxrate).
    let bpp = if hw { 0.08 } else { 0.055 };
    let mbps = (w / h as f64 * shown_h as f64 * shown_h as f64 * bpp * 60.0 * (fps as f64 / 60.0).powf(0.6)).max(1e6) / 1e6;
    let gb = mbps / 8.0 * (i.longest_secs.max(30) + 5) as f64 / 1000.0;
    let buffer = if i.ram_gb >= 16.0 && gb <= i.ram_gb * 0.10 {
        item("buffer", json!("ram"), "Your {ram} GB of RAM holds the whole buffer (at most {size} GB), so nothing is written to disk", json!({ "ram": i.ram_gb.round(), "size": (gb * 10.0).ceil() / 10.0 }))
    } else if i.ram_gb > 0.0 && i.ram_gb < 8.0 {
        item("buffer", json!("disk"), "With {ram} GB of RAM the buffer is best kept in files", json!({ "ram": i.ram_gb.round() }))
    } else {
        item("buffer", json!("mixed"), "The newest part stays in RAM and the rest goes to files, so long clips do not fill your {ram} GB of RAM", json!({ "ram": i.ram_gb.round() }))
    };
    out.push(buffer);
    out
}

pub fn rec_input(core: &Core) -> RecInput {
    let s: Settings = core.settings();
    let hw = win::hardware();
    let ffmpeg = util::find_exe(&s.ffmpeg_path, "ffmpeg");
    let mons = win::monitors();
    let m = mons.get(core::capture_monitor(&s, mons.len().max(1)) as usize).cloned();
    let border = border_info();
    RecInput {
        gpu: hw.gpu.map(|g| g.name).unwrap_or_default(),
        encoder: ffmpeg.as_ref().map(|f| recorder::resolve_encoder(f, "auto")).unwrap_or_else(|| hw.encoder.clone()),
        width: m.as_ref().map_or(1920, |m| m.width),
        height: m.as_ref().map_or(1080, |m| m.height),
        hz: m.as_ref().map_or(60, |m| m.hz),
        ram_gb: (ram_bytes() as f64 / (1u64 << 30) as f64 * 10.0).round() / 10.0,
        threads: cpu_threads(),
        build: hw.windows_build,
        has_wgc: ffmpeg.as_ref().is_some_and(|f| recorder::has_wgc(f)),
        border_ok: border.can_remove,
        longest_secs: s.max_clip_secs(),
    }
}

#[tauri::command(async)]
pub fn recommended_settings(core: tauri::State<'_, Arc<Core>>) -> Recommended {
    let input = rec_input(&core);
    Recommended { items: recommend(&input), input }
}

/* ---------- self-test (run by --polish-test) ---------- */

pub fn self_test_checks() -> Vec<(String, bool, String)> {
    let mut out: Vec<(String, bool, String)> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| out.push((name.to_string(), ok, detail));

    // Border setting.
    let b = border_decide(26100, "Allow", "Allow", "Allow");
    check("border: all allowed on Windows 11 -> can remove", b.can_remove && b.configurable, format!("{b:?}"));
    check("border: desktop apps denied -> cannot", !border_decide(26100, "Allow", "Deny", "Allow").can_remove, String::new());
    check("border: main switch denied -> cannot", !border_decide(26100, "Deny", "Allow", "").can_remove, String::new());
    check("border: device denied -> cannot", !border_decide(26100, "", "", "Deny").can_remove, String::new());
    check("border: nothing stored -> can (Windows' default is allow)", border_decide(22631, "", "", "").can_remove, String::new());
    check("border: Windows 10 19045 has no API -> cannot", !border_decide(19045, "Allow", "Allow", "Allow").can_remove, String::new());
    check("border: build 20348 has the API", border_decide(20348, "", "", "").configurable, String::new());
    let real = border_info();
    check("border: this PC's setting is read", real.build > 10000, format!("{real:?}"));

    // "auto" method with the border.
    use crate::polish::pick_method_border as pmb;
    check("auto + border cannot be removed -> dda", pmb("auto", true, 26100, false) == "dda", String::new());
    check("auto + border removable -> wgc", pmb("auto", true, 26100, true) == "wgc", String::new());
    check("wgc chosen by hand stays wgc with the border", pmb("wgc", true, 26100, false) == "wgc", String::new());
    check("method_why", method_why(true, 26100, false) == "border" && method_why(true, 19045, true) == "windows10" && method_why(false, 26100, true) == "no_wgc" && method_why(true, 26100, true).is_empty(), String::new());

    // ffmpeg log lines.
    use recorder::split_log_line as sl;
    check("log: warning recognised", sl("[Parsed_gfxcapture_0 @ 0000021] [warning] Failed setting border drawing mode") == (true, "[Parsed_gfxcapture_0 @ 0000021] Failed setting border drawing mode".to_string()), format!("{:?}", sl("[x @ 1] [warning] Failed setting border drawing mode")));
    check("log: error keeps the old text", sl("[libx264 @ 000001] [error] width not divisible by 2 (17x17)") == (false, "[libx264 @ 000001] width not divisible by 2 (17x17)".to_string()), String::new());
    check("log: x264's own line kept", sl("x264 [error]: invalid preset 'nonsense'") == (false, "x264 [error]: invalid preset 'nonsense'".to_string()), String::new());
    check("log: untagged line is an error", sl("Something broke") == (false, "Something broke".to_string()), String::new());

    // The capture command (built only, never run).
    if let Some(m) = win::monitors().first() {
        let cfg = core::capture_cfg_for(&Settings::default(), m, 30);
        let filt = |a: &[String]| a.iter().skip_while(|x| *x != "-filter_complex").nth(1).cloned().unwrap_or_default();
        let on = recorder::capture_args(&cfg, "nvenc", "wgc", false, true);
        let off = recorder::capture_args(&cfg, "nvenc", "wgc", false, false);
        let dda = recorder::capture_args(&cfg, "nvenc", "dda", false, true);
        check("args: display_border=0 on the Graphics Capture filter when ffmpeg has it", filt(&on).starts_with("gfxcapture=") && filt(&on).contains(":display_border=0") && !filt(&off).contains("display_border"), filt(&on));
        check("args: Desktop Duplication has no border option", !filt(&dda).contains("display_border"), filt(&dda));
        check("args: warnings are logged with their level", on.windows(2).any(|w| w[0] == "-loglevel" && w[1] == "level+warning"), String::new());
    }
    if let Some(ff) = util::find_exe("", "ffmpeg") {
        let res = recorder::resolve_method(&ff, "auto");
        check("this PC's ffmpeg has gfxcapture and its display_border option, auto resolves", recorder::gfx_caps(&ff) == (true, true) && (res == "wgc" || res == "dda"), res);
    }

    // Recommended settings.
    let base = RecInput { gpu: "NVIDIA GeForce RTX 5060 Ti".into(), encoder: "nvenc".into(), width: 2560, height: 1440, hz: 165, ram_gb: 32.0, threads: 12, build: 26100, has_wgc: true, border_ok: true, longest_secs: 120 };
    let get = |items: &[RecItem], k: &str| items.iter().find(|x| x.key == k).map(|x| x.value.clone()).unwrap_or(Value::Null);
    let r = recommend(&base);
    check("rec 1440p165 NVENC: native, 120 fps, high, auto, ram", get(&r, "height") == json!(0) && get(&r, "fps") == json!(120) && get(&r, "quality") == json!("high") && get(&r, "encoder") == json!("auto") && get(&r, "method") == json!("auto") && get(&r, "bitrate") == json!(0) && get(&r, "buffer") == json!("ram"), format!("{:?}", r.iter().map(|x| (&x.key, &x.value)).collect::<Vec<_>>()));
    let r = recommend(&RecInput { width: 3840, height: 2160, hz: 144, ..base.clone() });
    check("rec 4K144 NVENC: native at 60", get(&r, "height") == json!(0) && get(&r, "fps") == json!(60), format!("{} {}", get(&r, "height"), get(&r, "fps")));
    let r = recommend(&RecInput { encoder: "amf".into(), width: 1920, height: 1080, hz: 144, ..base.clone() });
    check("rec 1080p144 AMF: native at 120", get(&r, "height") == json!(0) && get(&r, "fps") == json!(120), format!("{} {}", get(&r, "height"), get(&r, "fps")));
    let r = recommend(&RecInput { encoder: "amf".into(), ..base.clone() });
    check("rec 1440p165 AMF: native at 60", get(&r, "height") == json!(0) && get(&r, "fps") == json!(60), format!("{} {}", get(&r, "height"), get(&r, "fps")));
    let r = recommend(&RecInput { encoder: "qsv".into(), width: 3840, height: 2160, hz: 60, ..base.clone() });
    check("rec 4K60 Quick Sync: 1600p at 60", get(&r, "height") == json!(1600) && get(&r, "fps") == json!(60), format!("{} {}", get(&r, "height"), get(&r, "fps")));
    let r = recommend(&RecInput { encoder: "x264".into(), gpu: String::new(), width: 1920, height: 1080, hz: 60, threads: 8, ram_gb: 8.0, ..base.clone() });
    check("rec x264 8 threads: 900p60, medium, mixed", get(&r, "height") == json!(900) && get(&r, "fps") == json!(60) && get(&r, "quality") == json!("medium") && get(&r, "buffer") == json!("mixed"), format!("{} {} {} {}", get(&r, "height"), get(&r, "fps"), get(&r, "quality"), get(&r, "buffer")));
    let r = recommend(&RecInput { encoder: "x264".into(), width: 1920, height: 1080, hz: 60, threads: 2, ram_gb: 4.0, ..base.clone() });
    check("rec x264 2 threads, 4 GB: 30 fps, disk", get(&r, "fps") == json!(30) && get(&r, "buffer") == json!("disk"), format!("{} {} {}", get(&r, "height"), get(&r, "fps"), get(&r, "buffer")));
    let r = recommend(&RecInput { hz: 60, ..base.clone() });
    check("rec 60 Hz screen: 60 fps", get(&r, "fps") == json!(60), String::new());
    let r = recommend(&RecInput { hz: 50, ..base.clone() });
    check("rec 50 Hz screen: 50 fps", get(&r, "fps") == json!(50), String::new());
    let r = recommend(&RecInput { ram_gb: 16.0, longest_secs: 1800, ..base.clone() });
    check("rec 30-minute clips on 16 GB: mixed", get(&r, "buffer") == json!("mixed"), String::new());
    let r = recommend(&RecInput { build: 19045, ..base.clone() });
    check("rec Windows 10: method auto (Desktop Duplication) with the reason", get(&r, "method") == json!("auto") && r.iter().any(|x| x.key == "method" && x.why.contains("Windows 10")), String::new());
    let heights_ok = [(2560u32, 1440u32), (3840, 2160), (1920, 1080), (1366, 768), (3440, 1440)].iter().all(|&(w, h)| {
        ["nvenc", "amf", "qsv", "x264"].iter().all(|e| {
            let r = recommend(&RecInput { encoder: e.to_string(), width: w, height: h, hz: 240, threads: 4, ..base.clone() });
            let v = get(&r, "height").as_u64().unwrap_or(1) as u32;
            v == 0 || (HEIGHTS.contains(&v) && v < h)
        })
    });
    check("rec heights are always ones the page offers", heights_ok, String::new());
    let ram = ram_bytes();
    check("this PC's RAM is read", ram > (1u64 << 30), format!("{:.1} GB", ram as f64 / (1u64 << 30) as f64));
    out
}
