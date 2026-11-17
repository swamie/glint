//! Small fixes from 0.1.11 user reports:
//! - the capture method "auto" on Windows 10 (no yellow Graphics Capture border),
//! - the taskbar / tray icon in the theme's accent color,
//! - a one-time repair of library thumbnails made with the wrong proportions,
//! and their self-test (`glint.exe --polish-test report.txt`).
use crate::core::Core;
use crate::library;
use crate::settings::Settings;
use crate::util;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/* ---------- capture method ---------- */

/// First Windows build (Windows 11) on which a Graphics Capture can be made without the yellow border.
pub const BORDERLESS_WGC_BUILD: u32 = 22000;

/// The capture method to use. "wgc" and "dda" are kept as the user chose them.
/// "auto" takes Windows Graphics Capture when ffmpeg has it, except on Windows
/// 10 (a build below 22000), where Windows draws a yellow border around the
/// captured screen that no program can turn off; there Desktop Duplication
/// (no border) is used. An unknown build (0) is treated as Windows 11.
pub fn pick_method(wanted: &str, has_wgc: bool, build: u32) -> &'static str {
    pick_method_border(wanted, has_wgc, build, true)
}

/// As `pick_method`, and "auto" also takes Desktop Duplication when Windows'
/// "Screenshot borders" setting does not let desktop apps hide the border
/// (`border_ok` false, from `capture2::border_info`).
pub fn pick_method_border(wanted: &str, has_wgc: bool, build: u32, border_ok: bool) -> &'static str {
    match wanted {
        "wgc" => "wgc",
        "dda" => "dda",
        _ if !has_wgc => "dda",
        _ if build != 0 && build < BORDERLESS_WGC_BUILD => "dda",
        _ if !border_ok => "dda",
        _ => "wgc",
    }
}

/* ---------- taskbar icon in the theme's color ---------- */

static STOCK_PNG: &[u8] = include_bytes!("../icons/icon.png");
/// The three colors the stock icon is drawn with: the dark tile, the big star, the small star.
const TILE: [f32; 3] = [10.0, 15.0, 24.0];
const STAR: [f32; 3] = [98.0, 182.0, 247.0];
const SPARK: [f32; 3] = [190.0, 226.0, 255.0];

/// "#rgb", "#rrggbb" or "rgb(r, g, b)" as RGB.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.is_ascii() {
            return None;
        }
        let v: Vec<u8> = match hex.len() {
            3 => hex.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8)).collect::<Option<_>>()?,
            6 | 8 => (0..3).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect::<Option<_>>()?,
            _ => return None,
        };
        return Some([v[0], v[1], v[2]]);
    }
    let inner = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split([',', ' ', '/']).filter(|p| !p.is_empty()).take(3).map(|p| p.trim().parse::<f32>().ok()).collect::<Option<_>>()?;
    (parts.len() == 3).then(|| [parts[0].clamp(0.0, 255.0) as u8, parts[1].clamp(0.0, 255.0) as u8, parts[2].clamp(0.0, 255.0) as u8])
}

/// The stock icon as RGBA pixels: (pixels, width, height).
pub fn stock_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let mut dec = png::Decoder::new(STOCK_PNG);
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    (rgba.len() == (w * h * 4) as usize).then_some((rgba, w, h))
}

/// Recolor the icon's stars: every pixel is read as the dark tile plus some
/// amount of the big star's color and of the small star's (that is how the
/// anti-aliased edges were drawn), and redrawn with `accent` for the big star
/// and `accent2` (or a lighter `accent`) for the small one. The tile and the
/// alpha channel stay as they are.
pub fn recolor(rgba: &[u8], accent: [u8; 3], accent2: Option<[u8; 3]>) -> Vec<u8> {
    let f = |c: [u8; 3]| [c[0] as f32, c[1] as f32, c[2] as f32];
    let a_new = f(accent);
    // The stock small star is the big one 59 % of the way to white.
    let s_new = accent2.map(f).unwrap_or_else(|| [0usize, 1, 2].map(|i| a_new[i] + (255.0 - a_new[i]) * 0.59));
    let u = [0usize, 1, 2].map(|i| STAR[i] - TILE[i]);
    let v = [0usize, 1, 2].map(|i| SPARK[i] - TILE[i]);
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
    let det = uu * vv - uv * uv;
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        let w = [0usize, 1, 2].map(|i| px[i] as f32 - TILE[i]);
        let (uw, vw) = (dot(u, w), dot(v, w));
        let mut a = ((vv * uw - uv * vw) / det).max(0.0);
        let mut l = ((uu * vw - uv * uw) / det).max(0.0);
        let sum = a + l;
        if sum > 1.0 {
            a /= sum;
            l /= sum;
        }
        if a + l < 0.004 {
            continue; // the tile itself (and fully transparent corners): unchanged
        }
        // Move the pixel by the change of each star color, in proportion: whatever
        // the three-color reading does not explain stays as it was.
        for i in 0..3 {
            px[i] = (px[i] as f32 + a * (a_new[i] - STAR[i]) + l * (s_new[i] - SPARK[i])).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The icon to show for these settings: `None` = the stock icon.
pub fn themed_rgba(s: &Settings) -> Option<(Vec<u8>, u32, u32)> {
    if !s.icon_theme {
        return None;
    }
    let accent = parse_color(&s.icon_accent)?;
    let (rgba, w, h) = stock_rgba()?;
    Some((recolor(&rgba, accent, parse_color(&s.icon_accent_2)), w, h))
}

/// `rgba` (w x h) scaled to n x n by averaging (alpha-weighted, so the transparent
/// corners do not darken the edges). Windows' own shrinking of a 256 px icon to a
/// 32 px taskbar button keeps every 8th pixel; this keeps the stars' shape.
pub fn scale_rgba(rgba: &[u8], w: u32, h: u32, n: u32) -> Vec<u8> {
    let (w, h, n) = (w as usize, h as usize, n.max(1) as usize);
    let mut out = vec![0u8; n * n * 4];
    if rgba.len() < w * h * 4 || w == 0 || h == 0 {
        return out;
    }
    for oy in 0..n {
        let (y0, y1) = (oy * h / n, ((oy + 1) * h / n).max(oy * h / n + 1).min(h));
        for ox in 0..n {
            let (x0, x1) = (ox * w / n, ((ox + 1) * w / n).max(ox * w / n + 1).min(w));
            let (mut acc, mut alpha, mut count) = ([0f64; 3], 0f64, 0f64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    let a = p[3] as f64;
                    for i in 0..3 {
                        acc[i] += p[i] as f64 * a;
                    }
                    alpha += a;
                    count += 1.0;
                }
            }
            let o = &mut out[(oy * n + ox) * 4..(oy * n + ox) * 4 + 4];
            if alpha > 0.0 {
                for i in 0..3 {
                    o[i] = (acc[i] / alpha).round().clamp(0.0, 255.0) as u8;
                }
            }
            o[3] = (alpha / count).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// The window's own icons, set straight through Win32. Tauri's `set_icon` only
/// changes the SMALL icon (title bar, Alt+Tab): the taskbar button draws the BIG
/// one, which stayed the exe's icon. That is why the taskbar showed the old color.
/// Both are made at the sizes Windows asks for. Returns whether both were set.
#[cfg(windows)]
fn set_window_icons(hwnd: isize, rgba: &[u8], w: u32, h: u32) -> bool {
    use std::sync::Mutex;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, GetSystemMetrics, SendMessageW, HICON, ICON_BIG, ICON_SMALL, SM_CXICON, SM_CXSMICON, WM_SETICON};
    // The icons Glint made last time (big, small): destroyed once replaced.
    static OURS: Mutex<[isize; 2]> = Mutex::new([0, 0]);
    let mut ours = OURS.lock().unwrap();
    let mut ok = true;
    for (slot, kind, metric, fallback) in [(0usize, ICON_BIG, SM_CXICON, 32), (1, ICON_SMALL, SM_CXSMICON, 16)] {
        let n = unsafe { GetSystemMetrics(metric) };
        let n = if (8..=256).contains(&n) { n as u32 } else { fallback };
        let px = scale_rgba(rgba, w, h, n);
        let bgra: Vec<u8> = px.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let mask = vec![0u8; n.div_ceil(16) as usize * 2 * n as usize]; // all 0: the alpha channel decides
        let Ok(icon) = (unsafe { CreateIcon(HINSTANCE::default(), n as i32, n as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) }) else {
            ok = false;
            continue;
        };
        unsafe { SendMessageW(HWND(hwnd as *mut _), WM_SETICON, WPARAM(kind as usize), LPARAM(icon.0 as isize)) };
        if ours[slot] != 0 {
            let _ = unsafe { DestroyIcon(HICON(ours[slot] as *mut _)) };
        }
        ours[slot] = icon.0 as isize;
    }
    ok
}

/// What `apply_icon` reached: the main window (open) and the tray icon.
#[derive(serde::Serialize, Default, Debug, Clone, Copy)]
pub struct IconApplied {
    pub window: bool,
    pub tray: bool,
}

/// Put the icon these settings ask for on the main window (taskbar button, title bar,
/// Alt+Tab) and the tray. `rebuild_tray`: also take the tray icon away and add it again
/// (the "Reload icon" button), for a tray that kept showing an old picture.
pub fn apply_icon_with(app: &tauri::AppHandle, s: &Settings, rebuild_tray: bool) -> IconApplied {
    use tauri::image::Image;
    use tauri::Manager;
    let img: Option<Image<'static>> = match themed_rgba(s) {
        Some((rgba, w, h)) => Some(Image::new_owned(rgba, w, h)),
        None => app.default_window_icon().map(|i| Image::new_owned(i.rgba().to_vec(), i.width(), i.height())),
    };
    let mut done = IconApplied::default();
    let Some(img) = img else { return done };
    if let Some(w) = app.get_webview_window("main") {
        #[cfg(windows)]
        {
            done.window = w.hwnd().is_ok_and(|h| set_window_icons(h.0 as isize, img.rgba(), img.width(), img.height()));
        }
        if !done.window {
            done.window = w.set_icon(img.clone()).is_ok();
        }
    }
    if let Some(tray) = app.tray_by_id("main") {
        done.tray = tray.set_icon(Some(img)).is_ok();
        if rebuild_tray && done.tray {
            // Removed and added again with the icon just set (tray-icon keeps it).
            let _ = tray.set_visible(false);
            let _ = tray.set_visible(true);
        }
    }
    done
}

/// `apply_icon_with` without rebuilding the tray: at start, and whenever the theme changes.
pub fn apply_icon(app: &tauri::AppHandle, s: &Settings) {
    apply_icon_with(app, s, false);
}

/// Store the icon setting and colors the UI computed (an accent that is not a color keeps the stored one).
fn store_icon(core: &Core, on: bool, accent: String, accent2: String) -> Settings {
    let mut s = core.settings.lock().unwrap();
    s.icon_theme = on;
    if parse_color(&accent).is_some() {
        s.icon_accent = accent;
        s.icon_accent_2 = if parse_color(&accent2).is_some() { accent2 } else { String::new() };
    }
    s.clone()
}

/// The UI computed the theme's colors (or the setting was turned off): store and show them.
#[tauri::command]
pub fn set_app_icon(app: tauri::AppHandle, core: tauri::State<'_, Arc<Core>>, on: bool, accent: String, accent2: String) {
    let s = store_icon(&core, on, accent, accent2);
    apply_icon(&app, &s);
}

/// Settings → "Reload icon": the same, right now, on the window, the taskbar and a rebuilt tray icon.
/// A pinned taskbar shortcut or a Start menu tile shows the exe's own icon: Windows does not let a
/// program change that, so it stays as it is.
#[tauri::command]
pub fn reload_app_icon(app: tauri::AppHandle, core: tauri::State<'_, Arc<Core>>, on: bool, accent: String, accent2: String) -> IconApplied {
    let s = store_icon(&core, on, accent, accent2);
    let done = apply_icon_with(&app, &s, true);
    util::log(&format!("icon reloaded: themed {}, window {}, tray {}", s.icon_theme && parse_color(&s.icon_accent).is_some(), done.window, done.tray));
    done
}

/* ---------- thumbnails ---------- */

/// Width and height of a JPEG, read from its frame header.
pub fn jpeg_size(path: &Path) -> Option<(u32, u32)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        if m == 0xFF || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]) as u32;
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// The shape a video is shown at: width × sample aspect ÷ height, turned for a 90° rotation.
pub fn display_ratio(ffmpeg: &Path, video: &Path) -> Option<f64> {
    let ffprobe = library::ffprobe_for(ffmpeg)?;
    let out = util::hidden_cmd(&ffprobe)
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,sample_aspect_ratio:stream_side_data=rotation", "-of", "default=nw=1"])
        .arg(video)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut w, mut h, mut sar, mut rot) = (0.0f64, 0.0f64, 1.0f64, 0.0f64);
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "width" if w == 0.0 => w = v.parse().unwrap_or(0.0),
            "height" if h == 0.0 => h = v.parse().unwrap_or(0.0),
            "sample_aspect_ratio" => {
                if let Some((n, d)) = v.split_once(':') {
                    let (n, d): (f64, f64) = (n.parse().unwrap_or(0.0), d.parse().unwrap_or(0.0));
                    if n > 0.0 && d > 0.0 {
                        sar = n / d;
                    }
                }
            }
            "rotation" => rot = v.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let r = w * sar / h;
    Some(if (rot.abs() - 90.0).abs() < 1.0 || (rot.abs() - 270.0).abs() < 1.0 { 1.0 / r } else { r })
}

/// A thumbnail must be redone: it is missing or unreadable, or its shape is
/// more than 3 % off the video's (a squashed anamorphic or rotated clip).
pub fn thumb_needs_redo(thumb: Option<(u32, u32)>, video_ratio: Option<f64>) -> bool {
    match (thumb, video_ratio) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some((w, h)), Some(r)) => ((w as f64 / h.max(1) as f64) / r - 1.0).abs() > 0.03,
    }
}

/// Once per data folder, in the background a little after start: redo the
/// thumbnails made before 0.1.11 whose shape does not match their clip.
pub fn spawn_thumb_repair(core: Arc<Core>) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(10));
        let marker = library::thumbs_dir().join(".fit-v2");
        if marker.is_file() {
            return;
        }
        let Some(ffmpeg) = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg") else { return };
        let fixed = repair_thumbs(&core, &ffmpeg);
        let _ = std::fs::write(&marker, b"thumbnails checked for shape\n");
        util::log(&format!("thumbnails: {fixed} redone to the clip's shape"));
        if fixed > 0 {
            core.emit("clips-changed", ());
        }
    });
}

/* ---------- thumbnails that are missing: tried again (0.1.41, item T) ----------
 * A clip whose thumbnail could not be made (the file not all there yet while it was copied or written, a slow or
 * sleeping drive, a file locked for a moment) used to keep the placeholder card for good. Now: every start, a
 * little after, in the background, the clips whose thumbnail is empty or whose thumbnail file is gone are tried
 * again (only those, one after the other, at most THUMB_RETRY_START per start), and the UI asks for one clip
 * (`thumb_retry`) when it draws its card without a picture. A clip is tried at most THUMB_TRIES_PER_DAY times a
 * day (`thumb-retry.json` in the data folder); a file with no picture at all (audio only) is marked once and never
 * tried again. A thumbnail that arrives is announced with "clips-changed" (the card swaps to it by itself). */

/// Tries per clip per day.
pub const THUMB_TRIES_PER_DAY: u32 = 3;
/// Clips tried at most per start.
pub const THUMB_RETRY_START: usize = 40;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct Tries {
    day: u64,
    count: u32,
    no_picture: bool,
    /// thumbs-refresh (0.1.46): the file's size and time when it was found to have no picture. A file that has
    /// changed since (it was still being written, or replaced) is looked at again: one reading of a file that was
    /// not all there yet no longer marks it for good. An entry from 0.1.41-0.1.45 (0, 0) is looked at again once.
    seen_size: u64,
    seen_ms: u64,
}

/// A file's (size, modified ms).
fn file_stamp(p: &Path) -> (u64, u64) {
    std::fs::metadata(p).map(|m| (m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0))).unwrap_or((0, 0))
}

static TRIES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tries_file() -> std::path::PathBuf {
    util::data_dir().join("thumb-retry.json")
}

fn load_tries() -> std::collections::HashMap<String, Tries> {
    std::fs::read_to_string(tries_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_tries(m: &std::collections::HashMap<String, Tries>) {
    if let Ok(t) = serde_json::to_string(m) {
        let _ = std::fs::write(tries_file(), t);
    }
}

/// What `retry_thumb` did.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Retried {
    /// A thumbnail was made (and the clip's entry points at it).
    Made,
    /// Nothing to do: it has one, the clip is gone, its file is gone, or its thumbnail is being made right now.
    Nothing,
    /// The file has no picture: kept as it is, never tried again.
    NoPicture,
    /// Tried enough today.
    Later,
    /// Tried, and it failed again (counted).
    Failed,
}

/// True if clip `c` shows no thumbnail: none, its file is gone, or (thumbs-refresh, 0.1.46) its file is empty or
/// not a JPEG (an ffmpeg ended while writing it): such a file used to count as "there", so it was never redone.
pub fn thumb_missing(c: &library::ClipMeta) -> bool {
    if c.draft {
        return false;
    }
    if c.thumb.trim().is_empty() {
        return true;
    }
    let mut head = [0u8; 2];
    let ok = std::fs::File::open(&c.thumb).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok();
    !(ok && head == [0xFF, 0xD8])
}

/// Try clip `id`'s missing thumbnail again (the rules above). `now_ms` is the clock (the self-test moves it).
pub fn retry_thumb(core: &Core, ffmpeg: &Path, id: &str, now_ms: u64) -> Retried {
    retry_thumb_as(core, ffmpeg, id, now_ms, false)
}

/// `retry_thumb`; `broken`: the window could not show the thumbnail it has (thumbs-refresh, 0.1.46: a picture the
/// window refuses is made again, whatever is on disk).
pub fn retry_thumb_as(core: &Core, ffmpeg: &Path, id: &str, now_ms: u64, broken: bool) -> Retried {
    let Some(c) = core.lib.lock().unwrap().get(id) else { return Retried::Nothing };
    if c.draft || !(broken || thumb_missing(&c)) || !Path::new(&c.path).is_file() || crate::import::thumb_pending(id) {
        return Retried::Nothing;
    }
    let day = now_ms / 86_400_000;
    let stamp = file_stamp(Path::new(&c.path));
    {
        let _g = TRIES_LOCK.lock().unwrap();
        let mut m = load_tries();
        let t = m.entry(id.to_string()).or_default();
        if t.no_picture && (t.seen_size, t.seen_ms) == stamp {
            return Retried::NoPicture;
        }
        t.no_picture = false; // the file changed since (or an old entry): looked at again
        if t.day != day {
            t.day = day;
            t.count = 0;
        }
        if t.count >= THUMB_TRIES_PER_DAY {
            return Retried::Later;
        }
        t.count += 1;
        save_tries(&m);
    }
    let p = library::probe(ffmpeg, Path::new(&c.path), false);
    if p.as_ref().is_some_and(|p| p.duration > 0.0 && p.video_streams == 0) {
        let _g = TRIES_LOCK.lock().unwrap();
        let mut m = load_tries();
        let t = m.entry(id.to_string()).or_default();
        t.no_picture = true;
        (t.seen_size, t.seen_ms) = stamp;
        save_tries(&m);
        util::log(&format!("thumbnail: clip {id} ({}) has no video stream; it keeps its card", library::file_name(Path::new(&c.path))));
        return Retried::NoPicture;
    }
    let dur = p.as_ref().map(|p| p.duration).filter(|d| *d > 0.0).unwrap_or(c.duration);
    // A new file name each time, so the window never shows a cached failure for it.
    let (th, _) = library::make_thumb_why(ffmpeg, Path::new(&c.path), &format!("{id}-r{now_ms}"), library::thumb_at(dur));
    if th.is_empty() {
        return Retried::Failed;
    }
    let old = c.thumb.clone();
    let probe = p.unwrap_or_default();
    let done = core.lib.lock().unwrap().update(id, |m| {
        m.thumb = th.clone();
        // A clip whose probe failed when it was added gets its numbers now too.
        if m.duration <= 0.0 && probe.duration > 0.0 {
            m.duration = probe.duration;
        }
        if m.width == 0 && probe.width > 0 {
            m.width = probe.width;
            m.height = probe.height;
            m.fps = probe.fps;
        }
    });
    if done.is_none() {
        let _ = std::fs::remove_file(&th);
        return Retried::Nothing;
    }
    if !old.is_empty() && old != th {
        let _ = std::fs::remove_file(&old);
    }
    {
        let _g = TRIES_LOCK.lock().unwrap();
        let mut m = load_tries();
        m.remove(id);
        save_tries(&m);
    }
    util::log(&format!("thumbnail: made again for clip {id}"));
    core.emit("clips-changed", id.to_string());
    Retried::Made
}

/// Every start, a little after, in the background: the clips without a thumbnail, tried again (THUMB_RETRY_START at most).
pub fn spawn_thumb_retry(core: Arc<Core>) {
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(15));
        let Some(ffmpeg) = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg") else { return };
        let ids: Vec<String> = core.lib.lock().unwrap().clips.iter().filter(|c| thumb_missing(c)).map(|c| c.id.clone()).take(THUMB_RETRY_START).collect();
        if ids.is_empty() {
            return;
        }
        let made = ids.iter().filter(|id| retry_thumb(&core, &ffmpeg, id, util::unix_ms()) == Retried::Made).count();
        util::log(&format!("thumbnails: {made} of {} missing ones made at start", ids.len()));
    });
}

/* ---------- thumbs-refresh (0.1.46): the user makes thumbnails again (F5 / Ctrl+F5 in the Library) ----------
 * `thumbs_refresh { all }` queues a job (kind "thumbs", one at a time, cancellable, progress in the jobs list):
 * all = false makes every MISSING thumbnail again (thumb_missing), ignoring the per-day limit and the "no picture"
 * memory; all = true makes every one again (wrong or black pictures). One ffmpeg at a time, below normal priority
 * (library::make_thumb_full). Each picture is announced with "clips-changed" as it arrives; at the end
 * "thumbs-done" { all, total, made, failed, reasons: [[reason, count]], cancelled }. Every failure is written to
 * glint.log with the clip's file name (never its folder), ffmpeg's exit code and its last line. */

/// What a refresh did.
#[derive(serde::Serialize, Debug, Default, Clone, PartialEq)]
pub struct Refreshed {
    pub all: bool,
    pub total: usize,
    pub made: usize,
    pub failed: usize,
    /// Why they failed, most frequent first: (reason, how many).
    pub reasons: Vec<(String, usize)>,
    pub cancelled: bool,
}

/// The clips a refresh does: every clip (all) or those without a thumbnail.
pub fn refresh_ids(core: &Core, all: bool) -> Vec<String> {
    core.lib.lock().unwrap().clips.iter().filter(|c| !c.draft && (all || thumb_missing(c))).map(|c| c.id.clone()).collect()
}

/// The reason a refresh reports for a thumbnail that could not be made.
fn refresh_reason(t: &library::Thumb) -> String {
    if t.timed_out {
        return format!("the file did not answer within {} s", library::THUMB_TIMEOUT.as_secs());
    }
    let e = t.error.trim();
    if e.starts_with("the ") || e.starts_with("ffmpeg could not be started") || e == "no picture came out" {
        return e.to_string();
    }
    format!("ffmpeg could not read the video: {}", e.lines().next().unwrap_or(""))
}

/// Make the thumbnails of clips `ids` again, one after the other (`ffmpeg` None: none can be made).
pub fn refresh_run(core: &Core, ffmpeg: Option<&Path>, ids: &[String], all: bool, cancel: &std::sync::atomic::AtomicBool, progress: &mut dyn FnMut(f32)) -> Refreshed {
    let mut r = Refreshed { all, total: ids.len(), ..Default::default() };
    let mut reasons: Vec<(String, usize)> = Vec::new();
    let mut fail = |reason: String, file: &str, detail: &str, r: &mut Refreshed| {
        r.failed += 1;
        util::log(&format!("thumbnails refresh: {file}: {detail}"));
        match reasons.iter_mut().find(|(x, _)| *x == reason) {
            Some(e) => e.1 += 1,
            None => reasons.push((reason, 1)),
        }
    };
    for (k, id) in ids.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            r.cancelled = true;
            break;
        }
        progress(100.0 * k as f32 / ids.len().max(1) as f32);
        let Some(c) = core.lib.lock().unwrap().get(id) else { r.total -= 1; continue };
        let file = library::file_name(Path::new(&c.path));
        if !Path::new(&c.path).is_file() {
            fail("the file is missing".into(), &file, "the file is missing", &mut r);
            continue;
        }
        let Some(ffmpeg) = ffmpeg else {
            fail("ffmpeg was not found".into(), &file, "ffmpeg was not found", &mut r);
            continue;
        };
        let p = library::probe(ffmpeg, Path::new(&c.path), false);
        if p.as_ref().is_some_and(|p| p.duration > 0.0 && p.video_streams == 0) {
            fail("the file has no video stream".into(), &file, "no video stream (ffprobe)", &mut r);
            continue;
        }
        let dur = p.as_ref().map(|p| p.duration).filter(|d| *d > 0.0).unwrap_or(c.duration);
        let t = library::make_thumb_full(ffmpeg, Path::new(&c.path), &format!("{id}-f{}", util::unix_ms()), library::thumb_at(dur));
        if t.path.is_empty() {
            fail(refresh_reason(&t), &file, &t.log_line(), &mut r);
            continue;
        }
        let old = c.thumb.clone();
        let th = t.path.clone();
        if core.lib.lock().unwrap().update(id, |m| m.thumb = th.clone()).is_none() {
            let _ = std::fs::remove_file(&th);
            r.total -= 1;
            continue;
        }
        if !old.is_empty() && old != th {
            let _ = std::fs::remove_file(&old);
        }
        {
            let _g = TRIES_LOCK.lock().unwrap();
            let mut m = load_tries();
            if m.remove(id).is_some() {
                save_tries(&m);
            }
        }
        r.made += 1;
        core.emit("clips-changed", id.to_string());
    }
    progress(100.0);
    reasons.sort_by(|a, b| b.1.cmp(&a.1));
    r.reasons = reasons;
    util::log(&format!("thumbnails refresh ({}): {} made, {} failed of {}{}", if all { "all" } else { "missing" }, r.made, r.failed, r.total, if r.cancelled { ", cancelled" } else { "" }));
    r
}

/// `thumbs_refresh`: queue the refresh. Returns the job (None when there is nothing to do) and how many clips it does.
pub fn refresh_thumbs(core: &Arc<Core>, all: bool) -> Result<(Option<crate::jobs::Job>, usize), String> {
    let n = refresh_ids(core, all).len();
    if n == 0 {
        return Ok((None, 0));
    }
    core.jobs.connect(core);
    let weak = Arc::downgrade(core);
    let run: crate::jobs::Runner = Box::new(move |cancel, progress| {
        let Some(core) = weak.upgrade() else { return Err("Glint is closing".into()) };
        let ffmpeg = util::find_exe(&core.settings().ffmpeg_path, "ffmpeg");
        let ids = refresh_ids(&core, all); // as they are when it starts
        let r = refresh_run(&core, ffmpeg.as_deref(), &ids, all, cancel, progress);
        core.emit("thumbs-done", serde_json::to_value(&r).unwrap_or_default());
        if r.cancelled { Err("Cancelled".into()) } else { Ok(String::new()) }
    });
    let job = core.jobs.enqueue("thumbs:refresh", if all { "Thumbnails (all)" } else { "Missing thumbnails" }, "thumbs", run)?;
    Ok((Some(job), n))
}

/// The `--import-test` checks of the retry (item T), on synthetic files in `dir`: (name, ok, detail).
pub fn thumb_retry_checks(core: &Core, ffmpeg: &Path, dir: &Path) -> Vec<(String, bool, String)> {
    let mut out = Vec::new();
    let _ = std::fs::remove_file(tries_file());
    let make = |name: &str, args: &[&str]| -> std::path::PathBuf {
        let p = dir.join(name);
        let _ = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-y"]).args(args).arg(&p).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        p
    };
    let video = make("thumb video.mp4", &["-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=3", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"]);
    let hevc10 = make("thumb hevc10 pq.mp4", &["-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=3", "-c:v", "libx265", "-preset", "ultrafast", "-pix_fmt", "yuv420p10le", "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc"]);
    let audio = make("thumb audio only.m4a", &["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=3", "-c:a", "aac"]);
    let odd = dir.join("Ünïcödé clip's 名前.mp4");
    let _ = std::fs::copy(&video, &odd);
    for (what, p) in [("h264", &video), ("hevc 10-bit with PQ tags", &hevc10), ("a non-ASCII name with an apostrophe", &odd)] {
        let (th, e) = library::make_thumb_why(ffmpeg, p, &format!("tt-{}", what.len()), 1.5);
        out.push((format!("thumbnail: {what}"), !th.is_empty() && Path::new(&th).is_file(), e));
    }
    let (th, _) = library::make_thumb_why(ffmpeg, &video, "tt-late", 99.0);
    out.push(("thumbnail: a seek past the end (a wrong duration) still gives the first frame".into(), !th.is_empty(), String::new()));
    let (th, e) = library::make_thumb_why(ffmpeg, &audio, "tt-audio", 1.5);
    out.push(("thumbnail: an audio-only file gives none, with ffmpeg's reason".into(), th.is_empty() && !e.trim().is_empty(), e));
    // A file not all there yet (a copy in progress): no thumbnail; once it is complete, the retry makes it.
    let full = std::fs::read(&video).unwrap_or_default();
    let growing = dir.join("thumb growing.mp4");
    let _ = std::fs::write(&growing, &full[..full.len() * 2 / 5]);
    let mut c = library::ClipMeta { id: "tt-grow".into(), path: growing.to_string_lossy().into_owned(), title: "growing".into(), duration: 3.0, kind: "import".into(), ..Default::default() };
    c.thumb = library::make_thumb(ffmpeg, &growing, &c.id, 1.5);
    out.push(("thumbnail: a file that is still being copied gets none at first".into(), c.thumb.is_empty(), String::new()));
    core.lib.lock().unwrap().add(c.clone());
    let a = library::ClipMeta { id: "tt-audio".into(), path: audio.to_string_lossy().into_owned(), title: "audio".into(), duration: 3.0, kind: "import".into(), ..Default::default() };
    core.lib.lock().unwrap().add(a);
    let t0 = 1_800_000_000_000u64;
    let r1 = retry_thumb(core, ffmpeg, "tt-grow", t0);
    out.push(("retry: still incomplete: failed, counted".into(), r1 == Retried::Failed, format!("{r1:?}")));
    let _ = std::fs::write(&growing, &full);
    let r2 = retry_thumb(core, ffmpeg, "tt-grow", t0 + 1000);
    let now = core.lib.lock().unwrap().get("tt-grow").unwrap_or_default();
    out.push(("retry: once the file is complete the thumbnail is made, and the clip points at it".into(), r2 == Retried::Made && !thumb_missing(&now), format!("{r2:?} {}", now.thumb)));
    let r3 = retry_thumb(core, ffmpeg, "tt-grow", t0 + 2000);
    out.push(("retry: a clip that has its thumbnail is left alone".into(), r3 == Retried::Nothing, format!("{r3:?}")));
    let ra = retry_thumb(core, ffmpeg, "tt-audio", t0);
    let ra2 = retry_thumb(core, ffmpeg, "tt-audio", t0 + 86_400_000 * 5);
    out.push(("retry: an audio-only file is marked once and never tried again".into(), ra == Retried::NoPicture && ra2 == Retried::NoPicture, format!("{ra:?} {ra2:?}")));
    // Backoff: a hopeless file is tried at most THUMB_TRIES_PER_DAY times a day.
    let junk = dir.join("thumb junk.mp4");
    let _ = std::fs::write(&junk, vec![0x5a_u8; 4096]);
    core.lib.lock().unwrap().add(library::ClipMeta { id: "tt-junk".into(), path: junk.to_string_lossy().into_owned(), title: "junk".into(), kind: "import".into(), ..Default::default() });
    let day: Vec<Retried> = (0..THUMB_TRIES_PER_DAY + 2).map(|i| retry_thumb(core, ffmpeg, "tt-junk", t0 + i as u64)).collect();
    let next = retry_thumb(core, ffmpeg, "tt-junk", t0 + 86_400_000);
    out.push((
        format!("retry: a hopeless file: {THUMB_TRIES_PER_DAY} tries a day, then later, and again the next day"),
        day.iter().filter(|r| **r == Retried::Failed).count() == THUMB_TRIES_PER_DAY as usize && day.last() == Some(&Retried::Later) && next == Retried::Failed,
        format!("{day:?} {next:?}"),
    ));

    // ---------- thumbs-refresh (0.1.46): the causes, each with its test ----------
    // A file that does not answer (a sleeping network drive, a cloud placeholder): ended after the limit, so it no
    // longer holds up every thumbnail after it.
    let t = std::time::Instant::now();
    let mut hang = util::hidden_cmd(ffmpeg);
    hang.args(["-hide_banner", "-loglevel", "error", "-re", "-f", "lavfi", "-i", "testsrc=s=64x64:r=10:d=120", "-f", "null", "-"]);
    let ran = library::run_limited(&mut hang, std::time::Duration::from_secs(1));
    out.push(("cause: a child that does not end is ended after the time limit".into(), ran.timed_out && ran.code.is_none() && t.elapsed().as_secs_f64() < 6.0, format!("{:.1} s", t.elapsed().as_secs_f64())));
    // The fallback chain: the wanted time, 0 s, the first key frame, a plain scale to yuvj420p.
    let late = library::make_thumb_full(ffmpeg, &video, "tt-chain", 99.0);
    out.push(("cause: a seek past the end (a wrong length, a very short clip): the 0 s attempt gives it".into(), !late.path.is_empty() && late.attempt == 2, format!("attempt {} {}", late.attempt, late.log_line())));
    let ok1 = library::make_thumb_full(ffmpeg, &video, "tt-chain1", 1.5);
    let jpeg = std::fs::read(&ok1.path).map(|b| b.starts_with(&[0xFF, 0xD8])).unwrap_or(false);
    out.push(("cause: ffmpeg hands the JPEG to Glint, which saves it (no write by ffmpeg into the thumbnails folder)".into(), ok1.attempt == 1 && jpeg, ok1.log_line()));
    // A path longer than 260 characters: given in the \\?\ form.
    let mut deep = dir.to_path_buf();
    while deep.to_string_lossy().len() < 270 {
        deep = deep.join("a long folder name of a clip library");
    }
    let deep_file = deep.join("deep clip.mp4");
    let _ = std::fs::create_dir_all(library::long_path(&deep));
    let _ = std::fs::copy(&video, library::long_path(&deep_file));
    let plain = util::hidden_cmd(ffmpeg).args(["-hide_banner", "-loglevel", "error", "-i"]).arg(&deep_file).args(["-frames:v", "1", "-f", "null", "-"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    let long = library::make_thumb_full(ffmpeg, &deep_file, "tt-long", 1.5);
    out.push((format!("cause: a path of {} characters gets its thumbnail (ffmpeg with the plain path: {})", deep_file.to_string_lossy().len(), if plain { "worked too" } else { "failed" }), !long.path.is_empty(), long.log_line()));
    let _ = std::fs::remove_dir_all(library::long_path(&dir.join("a long folder name of a clip library")));
    // An empty or broken thumbnail file used to count as "there", so it was never redone.
    let empty = dir.join("tt empty.jpg");
    let _ = std::fs::write(&empty, b"");
    let text = dir.join("tt text.jpg");
    let _ = std::fs::write(&text, b"not a picture");
    let with = |p: &Path| library::ClipMeta { thumb: p.to_string_lossy().into_owned(), ..Default::default() };
    out.push(("cause: an empty or non-JPEG thumbnail file counts as missing; a real one does not".into(), thumb_missing(&with(&empty)) && thumb_missing(&with(&text)) && !thumb_missing(&with(Path::new(&ok1.path))), String::new()));
    // A picture the window could not show is made again when the card says so (thumb_retry broken).
    let shown = dir.join("thumb shown.mp4");
    let _ = std::fs::copy(&video, &shown);
    core.lib.lock().unwrap().add(library::ClipMeta { id: "tt-shown".into(), path: shown.to_string_lossy().into_owned(), thumb: ok1.path.clone(), duration: 3.0, kind: "import".into(), ..Default::default() });
    let b1 = retry_thumb_as(core, ffmpeg, "tt-shown", t0, false);
    let b2 = retry_thumb_as(core, ffmpeg, "tt-shown", t0 + 1, true);
    out.push(("cause: a thumbnail the window refuses is made again on request (broken); one that is fine is left alone".into(), b1 == Retried::Nothing && b2 == Retried::Made, format!("{b1:?} {b2:?}")));
    // The "no picture" memory: a file found without a video stream and changed since is looked at again.
    let _ = std::fs::copy(&video, &audio); // the same path, now a video (it was still being written, or replaced)
    let rn = retry_thumb(core, ffmpeg, "tt-audio", t0 + 86_400_000 * 6);
    out.push(("cause: the no-picture memory is not for good: the file changed, it is looked at again and gets its thumbnail".into(), rn == Retried::Made, format!("{rn:?}")));
    // The per-day limit resets the next day (above), and a refresh ignores it and the memory.

    // The refresh: a clip with its thumbnail, one without, an unreadable one, one whose file is gone, one with no picture.
    let gone = dir.join("thumb gone.mp4");
    let noaud = make("thumb only sound.m4a", &["-f", "lavfi", "-i", "sine=frequency=330:sample_rate=48000:duration=2", "-c:a", "aac"]);
    let add = |id: &str, p: &Path, thumb: &str| core.lib.lock().unwrap().add(library::ClipMeta { id: id.into(), path: p.to_string_lossy().into_owned(), title: id.into(), thumb: thumb.into(), duration: 3.0, kind: "import".into(), ..Default::default() });
    let have = library::make_thumb(ffmpeg, &video, "tt-r-have-old", 1.5);
    add("tt-r-have", &video, &have);
    add("tt-r-none", &shown, "");
    add("tt-r-junk", &junk, "");
    add("tt-r-gone", &gone, "");
    add("tt-r-sound", &noaud, "");
    {
        let _g = TRIES_LOCK.lock().unwrap();
        let mut m = load_tries();
        m.insert("tt-r-none".into(), Tries { day: t0 / 86_400_000 + 100_000, count: 99, ..Default::default() }); // over the per-day limit
        save_tries(&m);
    }
    let missing = refresh_ids(core, false);
    let mine = ["tt-r-have", "tt-r-none", "tt-r-junk", "tt-r-gone", "tt-r-sound"];
    out.push(("refresh: the missing ones are the clips without a thumbnail only".into(), ["tt-r-none", "tt-r-junk", "tt-r-gone", "tt-r-sound"].iter().all(|x| missing.iter().any(|m| m == x)) && !missing.iter().any(|m| m == "tt-r-have"), format!("{missing:?}")));
    let ids: Vec<String> = ["tt-r-none", "tt-r-junk", "tt-r-gone", "tt-r-sound"].iter().map(|s| s.to_string()).collect();
    let no = std::sync::atomic::AtomicBool::new(false);
    let mut pcts: Vec<f32> = Vec::new();
    let r = refresh_run(core, Some(ffmpeg), &ids, false, &no, &mut |p| pcts.push(p));
    let reasons: Vec<String> = r.reasons.iter().map(|(x, n)| format!("{x} ({n})")).collect();
    out.push((
        "refresh (missing): 1 made (past the per-day limit), 3 failed with their reasons grouped".into(),
        r.total == 4 && r.made == 1 && r.failed == 3 && r.reasons.len() == 3 && r.reasons.iter().any(|(x, n)| x == "the file is missing" && *n == 1) && r.reasons.iter().any(|(x, n)| x == "the file has no video stream" && *n == 1) && r.reasons.iter().any(|(x, _)| x.starts_with("ffmpeg could not read the video: ")) && pcts.last() == Some(&100.0),
        format!("{} made, {} failed: {}", r.made, r.failed, reasons.join("; ")),
    ));
    out.push(("refresh: the one made points at its new picture".into(), core.lib.lock().unwrap().get("tt-r-none").is_some_and(|c| !thumb_missing(&c)), String::new()));
    let before = core.lib.lock().unwrap().get("tt-r-have").map(|c| c.thumb).unwrap_or_default();
    let all_ids: Vec<String> = vec!["tt-r-have".into(), "tt-r-junk".into()];
    let ra = refresh_run(core, Some(ffmpeg), &all_ids, true, &no, &mut |_| {});
    let after = core.lib.lock().unwrap().get("tt-r-have").map(|c| c.thumb).unwrap_or_default();
    out.push(("refresh (all): a clip that had a thumbnail gets a new one; the old file is removed".into(), ra.made == 1 && ra.failed == 1 && after != before && Path::new(&after).is_file() && !Path::new(&before).exists(), format!("{ra:?}")));
    let rf = refresh_run(core, None, &all_ids, true, &no, &mut |_| {});
    out.push(("refresh: without ffmpeg every one fails with that reason".into(), rf.failed == 2 && rf.reasons == vec![("ffmpeg was not found".to_string(), 2)], format!("{rf:?}")));
    let yes = std::sync::atomic::AtomicBool::new(true);
    let rc = refresh_run(core, Some(ffmpeg), &all_ids, true, &yes, &mut |_| {});
    out.push(("refresh: cancelled before it starts makes nothing".into(), rc.cancelled && rc.made == 0 && rc.failed == 0, format!("{rc:?}")));
    for id in mine.iter().chain(["tt-shown"].iter()) {
        if let Some(m) = core.lib.lock().unwrap().remove(id) {
            if !m.thumb.is_empty() {
                let _ = std::fs::remove_file(&m.thumb);
            }
        }
    }
    for id in ["tt-grow", "tt-audio", "tt-junk"] {
        if let Some(m) = core.lib.lock().unwrap().remove(id) {
            if !m.thumb.is_empty() {
                let _ = std::fs::remove_file(&m.thumb);
            }
        }
    }
    let _ = std::fs::remove_file(tries_file());
    out
}

fn repair_thumbs(core: &Core, ffmpeg: &Path) -> usize {
    let clips: Vec<(String, String, String, f64)> = core.lib.lock().unwrap().clips.iter().map(|c| (c.id.clone(), c.path.clone(), c.thumb.clone(), c.duration)).collect();
    let mut fixed = 0;
    for (id, path, thumb, duration) in clips {
        let size = if thumb.is_empty() { None } else { jpeg_size(Path::new(&thumb)) };
        if !thumb_needs_redo(size, display_ratio(ffmpeg, Path::new(&path))) {
            continue;
        }
        // A new file name, so the window does not show the old picture from its cache.
        let new = library::make_thumb(ffmpeg, Path::new(&path), &format!("{id}-fit"), library::thumb_at(duration));
        if new.is_empty() || new == thumb {
            continue;
        }
        if core.lib.lock().unwrap().update(&id, |c| c.thumb = new.clone()).is_some() {
            if !thumb.is_empty() {
                let _ = std::fs::remove_file(&thumb);
            }
            fixed += 1;
        } else {
            let _ = std::fs::remove_file(&new);
        }
    }
    fixed
}

/* ---------- self-test ---------- */

struct Report {
    lines: Vec<String>,
    pass: usize,
    fail: usize,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.pass += 1;
        } else {
            self.fail += 1;
        }
        self.lines.push(format!("{} {name} ({detail})", if ok { "PASS" } else { "FAIL" }));
    }
}

/// The most common color among the icon's opaque pixels that are not the dark tile.
fn dominant_star(rgba: &[u8]) -> Option<[u8; 3]> {
    let mut counts: std::collections::HashMap<[u8; 3], usize> = Default::default();
    for p in rgba.chunks_exact(4).filter(|p| p[3] == 255) {
        let c = [p[0], p[1], p[2]];
        if c != [TILE[0] as u8, TILE[1] as u8, TILE[2] as u8] {
            *counts.entry(c).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

fn near(a: [u8; 3], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (a[i] as i32 - b[i] as i32).abs() <= tol)
}

/// `set_window_icons` on a window that is created hidden (no WS_VISIBLE, never shown,
/// no taskbar button), read back with WM_GETICON, then destroyed.
#[cfg(windows)]
fn icon_on_hidden_window(rgba: &[u8], w: u32, h: u32) -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, IsWindowVisible, SendMessageW, HMENU, ICON_BIG, ICON_SMALL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETICON};
    let Ok(hwnd) = (unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WINDOW_STYLE(0), 0, 0, 10, 10, HWND::default(), HMENU::default(), HINSTANCE::default(), None) }) else { return false };
    let set = set_window_icons(hwnd.0 as isize, rgba, w, h);
    let got = |kind: u32| unsafe { SendMessageW(hwnd, WM_GETICON, WPARAM(kind as usize), LPARAM(0)) }.0 != 0;
    let ok = set && got(ICON_BIG) && got(ICON_SMALL) && !unsafe { IsWindowVisible(hwnd) }.as_bool();
    // Twice: the icons of the first call are destroyed and replaced without a failure.
    let again = set_window_icons(hwnd.0 as isize, rgba, w, h) && got(ICON_BIG);
    let _ = unsafe { DestroyWindow(hwnd) };
    ok && again
}

pub fn self_test(report_path: &str) {
    let mut r = Report { lines: Vec::new(), pass: 0, fail: 0 };

    // 1. Capture method.
    let cases: [(&str, bool, u32, &str); 12] = [
        ("auto", true, 19045, "dda"),
        ("auto", true, 22000, "wgc"),
        ("auto", true, 26100, "wgc"),
        ("auto", true, 0, "wgc"),
        ("auto", false, 26100, "dda"),
        ("auto", false, 19045, "dda"),
        ("wgc", true, 19045, "wgc"),
        ("wgc", false, 26100, "wgc"),
        ("dda", true, 26100, "dda"),
        ("dda", true, 19045, "dda"),
        ("", true, 19041, "dda"),
        ("", true, 22631, "wgc"),
    ];
    for (wanted, has, build, want) in cases {
        let got = pick_method(wanted, has, build);
        r.check(&format!("method {wanted:?} wgc={has} build {build} -> {want}"), got == want, got);
    }
    let build = crate::win::windows_build();
    r.check("this PC's Windows build is read", build > 10000, &build.to_string());
    // 0.1.16: the screenshot border setting, ffmpeg's log lines, recommended settings (capture2.rs).
    for (name, ok, detail) in crate::capture2::self_test_checks() {
        r.check(&name, ok, &detail);
    }
    // 0.1.20: Blur's GPU encoding default and the GPU type fitted to this PC's cards (blur.rs).
    for (name, ok, detail) in crate::blur::gpu_self_checks() {
        r.check(&name, ok, &detail);
    }
    // 0.1.31: a custom clip sound is cut to its part and set to its volume (sound.rs). Nothing is played.
    for (name, ok, detail) in crate::sound::self_checks() {
        r.check(&name, ok, &detail);
    }

    // 2. Icon.
    match stock_rgba() {
        None => r.check("stock icon decodes", false, "png decode failed"),
        Some((stock, w, h)) => {
            r.check("stock icon decodes", w == 256 && h == 256, &format!("{w}x{h}"));
            let star = dominant_star(&stock);
            r.check("stock star color found", star == Some([98, 182, 247]), &format!("{star:?}"));
            let same = recolor(&stock, [98, 182, 247], Some([190, 226, 255]));
            let diff = stock.iter().zip(&same).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            r.check("recolor to the stock colors changes nothing", diff <= 2, &format!("largest change {diff}"));
            for (name, accent) in [("crimson", "#ff5c6c"), ("emerald", "#4ade80"), ("gold", "#f2c94c"), ("mono", "#f2f2f2"), ("amethyst", "rgb(178, 140, 255)")] {
                let acc = parse_color(accent).unwrap_or([0, 0, 0]);
                let out = recolor(&stock, acc, None);
                let alpha_kept = out.len() == stock.len() && out.iter().skip(3).step_by(4).eq(stock.iter().skip(3).step_by(4));
                let tile_kept = out[(128 * 256 + 20) * 4..(128 * 256 + 20) * 4 + 3] == stock[(128 * 256 + 20) * 4..(128 * 256 + 20) * 4 + 3];
                let dom = dominant_star(&out);
                r.check(&format!("icon in {name}: same size, alpha and tile kept, star is the accent"), alpha_kept && tile_kept && dom.is_some_and(|d| near(d, acc, 3)), &format!("dominant {dom:?}, accent {acc:?}"));
                // A copy to look at, beside the report.
                let file = Path::new(report_path).with_file_name(format!("icon-{name}.png"));
                if let Ok(f) = std::fs::File::create(&file) {
                    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    let _ = enc.write_header().and_then(|mut wr| wr.write_image_data(&out));
                }
            }
            let mut s = Settings::default();
            s.icon_accent = "#ff5c6c".into();
            r.check("setting off: stock icon", themed_rgba(&s).is_none(), "");
            s.icon_theme = true;
            r.check("setting on: themed icon", themed_rgba(&s).is_some_and(|(px, ww, hh)| ww == w && hh == h && dominant_star(&px).is_some_and(|d| near(d, [255, 92, 108], 3))), "");
            s.icon_accent = "not a color".into();
            r.check("setting on with no color yet: stock icon", themed_rgba(&s).is_none(), "");
            // 0.1.18: the window's big and small icons are made at the sizes Windows asks for.
            let crimson = recolor(&stock, [255, 92, 108], None);
            for n in [16u32, 20, 24, 32, 48, 64] {
                let px = scale_rgba(&crimson, w, h, n);
                let corner_clear = stock[3] == 255 || px[3] < 128; // a transparent corner stays (mostly) transparent
                let (tx, ty) = (n * 40 / 256, n * 215 / 256); // bare tile, below-left of the big star's tips
                let mid = &px[((ty * n + tx) * 4) as usize..((ty * n + tx) * 4 + 3) as usize];
                let dom = dominant_star(&px);
                r.check(&format!("icon at {n} px: right size, corner kept, tile and star colors"), px.len() == (n * n * 4) as usize && corner_clear && near([mid[0], mid[1], mid[2]], [10, 15, 24], 6) && dom.is_some_and(|d| near(d, [255, 92, 108], 24)), &format!("corner alpha {}, tile {mid:?}, dominant {dom:?}", px[3]));
            }
            let flat = vec![200u8, 100, 50, 255].repeat(4);
            r.check("scaling a flat color keeps it; transparent stays transparent", scale_rgba(&flat, 2, 2, 1) == vec![200, 100, 50, 255] && scale_rgba(&[9, 9, 9, 0, 9, 9, 9, 0], 2, 1, 1)[3] == 0 && scale_rgba(&[], 0, 0, 4).len() == 64, "");
            let half = [255u8, 0, 0, 255, 0, 0, 255, 0]; // a red pixel beside a fully transparent blue one
            let s1 = scale_rgba(&half, 2, 1, 1);
            r.check("a transparent pixel adds no color to its neighbour", s1[0] == 255 && s1[2] == 0 && (127..=128).contains(&s1[3]), &format!("{s1:?}"));
            // The Windows part on a hidden window of our own: never shown, destroyed again.
            #[cfg(windows)]
            r.check("Win32 icons set on a hidden window (big and small)", icon_on_hidden_window(&crimson, w, h), "");
        }
    }
    {
        // The main window's frame subclass (win::calm_frame): which messages it answers itself.
        let m = crate::win::frame_message;
        r.check("frame: the themed caption and frame draws are swallowed", m(0x00AE, 5) == (Some(0), 5) && m(0x00AF, 7) == (Some(0), 7), "");
        r.check("frame: activation is passed on without a repaint of the frame (lParam -1)", m(0x0086, 0) == (None, -1), "");
        r.check("frame: every other message passes unchanged", m(0x0005, 42) == (None, 42) && m(0x0083, 9) == (None, 9) && m(0x0085, 1) == (None, 1), "");
    }
    r.check("color parsing", parse_color("#abc") == Some([170, 187, 204]) && parse_color(" #62B6F7 ") == Some([98, 182, 247]) && parse_color("rgb(1, 2, 3)") == Some([1, 2, 3]) && parse_color("blue").is_none(), "");

    // 3. Thumbnails (needs ffmpeg).
    r.check("redo: missing thumbnail", thumb_needs_redo(None, Some(16.0 / 9.0)), "");
    r.check("redo: 4:3 thumbnail of a 16:9 clip", thumb_needs_redo(Some((480, 360)), Some(16.0 / 9.0)), "");
    r.check("keep: 480x270 thumbnail of a 16:9 clip", !thumb_needs_redo(Some((480, 270)), Some(16.0 / 9.0)), "");
    r.check("keep: unknown clip shape", !thumb_needs_redo(Some((480, 360)), None), "");
    let dir = std::env::temp_dir().join(format!("glint-polish-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    match util::find_exe("", "ffmpeg") {
        None => r.check("ffmpeg found", false, "not on PATH"),
        Some(ff) => {
            // name, size, extra filter, codec, expected display shape
            let srcs: [(&str, &str, &str, &[&str], f64); 7] = [
                ("wide", "2560x1440", "", &["-c:v", "libx264", "-pix_fmt", "yuv420p"], 16.0 / 9.0),
                ("ultrawide", "3440x1440", "", &["-c:v", "libx264", "-pix_fmt", "yuv420p"], 3440.0 / 1440.0),
                ("stretched43", "1440x1080", "", &["-c:v", "libx264", "-pix_fmt", "yuv420p"], 4.0 / 3.0),
                ("anamorphic", "1440x1080", ",setsar=4/3", &["-c:v", "libx264", "-pix_fmt", "yuv420p"], 16.0 / 9.0),
                ("portrait_hevc10", "1080x1920", "", &["-c:v", "libx265", "-pix_fmt", "yuv420p10le", "-x265-params", "log-level=none"], 1080.0 / 1920.0),
                ("odd", "1366x769", "", &["-c:v", "libx264", "-pix_fmt", "yuv444p"], 1366.0 / 769.0),
                // A phone-style clip: stored landscape with a 90° display matrix (made from "wide" below).
                ("rotated", "", "", &[], 1440.0 / 2560.0),
            ];
            for (name, size, extra, codec, want) in srcs {
                let file = dir.join(format!("{name}.mp4"));
                let mut cmd = util::hidden_cmd(&ff);
                cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
                if name == "rotated" {
                    cmd.args(["-display_rotation:v:0", "90", "-i"]).arg(dir.join("wide.mp4")).args(["-c", "copy"]);
                } else {
                    cmd.args(["-f", "lavfi", "-i", &format!("testsrc2=s={size}:d=4:r=30"), "-vf", &format!("format=yuv420p{extra}")]).args(codec);
                }
                cmd.arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                if !cmd.status().is_ok_and(|s| s.success()) || !file.is_file() {
                    r.check(&format!("thumb {name}: source made"), false, "ffmpeg could not make it");
                    continue;
                }
                let ratio = display_ratio(&ff, &file);
                r.check(&format!("thumb {name}: display shape probed"), ratio.is_some_and(|x| (x / want - 1.0).abs() < 0.01), &format!("{ratio:?} vs {want:.4}"));
                let started = std::time::Instant::now();
                let thumb = library::make_thumb(&ff, &file, &format!("polish-test-{name}-{}", std::process::id()), library::thumb_at(4.0));
                let ms = started.elapsed().as_millis();
                let sz = jpeg_size(Path::new(&thumb));
                let ok = sz.is_some_and(|(w, h)| {
                    let shape_ok = ((w as f64 / h as f64) / want - 1.0).abs() < 0.02;
                    shape_ok && w.max(h) <= 640 && (w == 640 || h == 640) && w % 2 == 0 && h % 2 == 0
                });
                r.check(&format!("thumb {name}: whole frame at the right shape, fits 640"), ok, &format!("{sz:?} in {ms} ms"));
                r.check(&format!("thumb {name}: shape check says keep"), !thumb_needs_redo(sz, ratio), "");
                let _ = std::fs::remove_file(&thumb);
            }
            // The old filter on the anamorphic source: the repair must catch it.
            let file = dir.join("anamorphic.mp4");
            let old = dir.join("old.jpg");
            let _ = util::hidden_cmd(&ff).args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "2", "-i"]).arg(&file).args(["-frames:v", "1", "-vf", "scale=480:-2"]).arg(&old).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            let sz = jpeg_size(&old);
            r.check("old filter on an anamorphic clip is caught by the repair", sz == Some((480, 360)) && thumb_needs_redo(sz, display_ratio(&ff, &file)), &format!("{sz:?}"));
            r.check("thumbnail time is the middle", library::thumb_at(30.0) == 15.0 && library::thumb_at(600.0) == 300.0 && library::thumb_at(f64::NAN) == 0.0, "");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);

    r.lines.push(format!("TOTAL {} pass, {} fail", r.pass, r.fail));
    let _ = std::fs::write(report_path, r.lines.join("\n"));
}
