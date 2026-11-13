//! Small fixes from 0.1.11 user reports:
//! - the capture method "auto" on Windows 10 (no yellow Graphics Capture border),
//! - the taskbar / tray icon in the theme's accent colour,
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
    match wanted {
        "wgc" => "wgc",
        "dda" => "dda",
        _ if !has_wgc => "dda",
        _ if build != 0 && build < BORDERLESS_WGC_BUILD => "dda",
        _ => "wgc",
    }
}

/* ---------- taskbar icon in the theme's colour ---------- */

static STOCK_PNG: &[u8] = include_bytes!("../icons/icon.png");
/// The three colours the stock icon is drawn with: the dark tile, the big star, the small star.
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

/// Recolour the icon's stars: every pixel is read as the dark tile plus some
/// amount of the big star's colour and of the small star's (that is how the
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
        // Move the pixel by the change of each star colour, in proportion: whatever
        // the three-colour reading does not explain stays as it was.
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

/// Put the icon these settings ask for on the main window (taskbar, Alt+Tab) and the tray.
pub fn apply_icon(app: &tauri::AppHandle, s: &Settings) {
    use tauri::image::Image;
    use tauri::Manager;
    let img: Option<Image<'static>> = match themed_rgba(s) {
        Some((rgba, w, h)) => Some(Image::new_owned(rgba, w, h)),
        None => app.default_window_icon().map(|i| Image::new_owned(i.rgba().to_vec(), i.width(), i.height())),
    };
    let Some(img) = img else { return };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_icon(img.clone());
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(img));
    }
}

/// The UI computed the theme's colours (or the setting was turned off): store and show them.
#[tauri::command]
pub fn set_app_icon(app: tauri::AppHandle, core: tauri::State<'_, Arc<Core>>, on: bool, accent: String, accent2: String) {
    let s = {
        let mut s = core.settings.lock().unwrap();
        s.icon_theme = on;
        if parse_color(&accent).is_some() {
            s.icon_accent = accent;
            s.icon_accent_2 = if parse_color(&accent2).is_some() { accent2 } else { String::new() };
        }
        s.clone()
    };
    apply_icon(&app, &s);
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

/// The most common colour among the icon's opaque pixels that are not the dark tile.
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

    // 2. Icon.
    match stock_rgba() {
        None => r.check("stock icon decodes", false, "png decode failed"),
        Some((stock, w, h)) => {
            r.check("stock icon decodes", w == 256 && h == 256, &format!("{w}x{h}"));
            let star = dominant_star(&stock);
            r.check("stock star colour found", star == Some([98, 182, 247]), &format!("{star:?}"));
            let same = recolor(&stock, [98, 182, 247], Some([190, 226, 255]));
            let diff = stock.iter().zip(&same).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            r.check("recolour to the stock colours changes nothing", diff <= 2, &format!("largest change {diff}"));
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
            s.icon_accent = "not a colour".into();
            r.check("setting on with no colour yet: stock icon", themed_rgba(&s).is_none(), "");
        }
    }
    r.check("colour parsing", parse_color("#abc") == Some([170, 187, 204]) && parse_color(" #62B6F7 ") == Some([98, 182, 247]) && parse_color("rgb(1, 2, 3)") == Some([1, 2, 3]) && parse_color("blue").is_none(), "");

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
