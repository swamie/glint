//! A small live picture of one monitor for the monitor picker (0.1.41, ui/monpick.js).
//!
//! `monitor_preview { index, width }` copies that monitor's current picture from the screen with
//! plain GDI (StretchBlt from the desktop's DC into a small DIB, about 320 px wide, halftone
//! scaling) and returns it as a PNG data URL. The UI asks for it about once a second while a
//! picker is on screen, and never otherwise. The monitor is found by its handle, as the capture
//! names it (`win::monitor_source`, 0.1.35), so the picture is the monitor of that number.
//! Exclusive-fullscreen games can come out black: GDI sees what the desktop composes.
//!
//! The self-test (`--monprev-test`) never reads the screen: the same scaling runs on a synthetic
//! in-memory bitmap, and the encoding and sizes are checked on it.
use crate::win;
use std::ffi::c_void;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HALFTONE, HDC, SRCCOPY,
};

/// The preview's size for a monitor of `w` x `h` at `width` pixels wide (64..=640), keeping its shape.
pub fn preview_size(w: i32, h: i32, width: u32) -> (i32, i32) {
    let dw = width.clamp(64, 640) as i32;
    if w <= 0 || h <= 0 {
        return (dw, dw * 9 / 16);
    }
    let dh = ((dw as f64 * h as f64 / w as f64).round() as i32).clamp(16, 1280);
    (dw, dh)
}

/// A 32-bit top-down DIB section selected into a new memory DC: (dc, bitmap, its pixels, old object).
unsafe fn dib(w: i32, h: i32) -> Option<(HDC, windows::Win32::Graphics::Gdi::HBITMAP, *mut u8, windows::Win32::Graphics::Gdi::HGDIOBJ)> {
    let dc = CreateCompatibleDC(None);
    if dc.is_invalid() {
        return None;
    }
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let Ok(bmp) = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
        let _ = DeleteDC(dc);
        return None;
    };
    if bits.is_null() {
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        return None;
    }
    let old = SelectObject(dc, bmp);
    Some((dc, bmp, bits as *mut u8, old))
}

unsafe fn free(dc: HDC, bmp: windows::Win32::Graphics::Gdi::HBITMAP, old: windows::Win32::Graphics::Gdi::HGDIOBJ) {
    SelectObject(dc, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(dc);
}

/// Scale the rectangle (x, y, w, h) of `src` to `dw` x `dh` with halftone: BGRA pixels, top row first.
unsafe fn stretch_from(src: HDC, rect: (i32, i32, i32, i32), dw: i32, dh: i32) -> Option<Vec<u8>> {
    let (dc, bmp, bits, old) = dib(dw, dh)?;
    SetStretchBltMode(dc, HALFTONE);
    let _ = SetBrushOrgEx(dc, 0, 0, None);
    let ok = StretchBlt(dc, 0, 0, dw, dh, src, rect.0, rect.1, rect.2, rect.3, SRCCOPY).as_bool();
    let _ = GdiFlush();
    let out = ok.then(|| std::slice::from_raw_parts(bits, (dw * dh * 4) as usize).to_vec());
    free(dc, bmp, old);
    out
}

/// BGRA pixels as an RGB PNG.
pub fn png_rgb(bgra: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if bgra.len() < (w * h * 4) as usize {
        return None;
    }
    let rgb: Vec<u8> = bgra.chunks_exact(4).take((w * h) as usize).flat_map(|p| [p[2], p[1], p[0]]).collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.write_header().ok()?.write_image_data(&rgb).ok()?;
    }
    Some(out)
}

/// Standard base64 with padding.
pub fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// The rectangle of monitor `hmonitor` on the virtual desktop.
fn monitor_rect(hmonitor: u64) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe { GetMonitorInfoW(HMONITOR(hmonitor as usize as *mut c_void), &mut mi).as_bool() }.then(|| {
        let r = mi.rcMonitor;
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    })
}

/// The picture of monitor `index` (`win::monitors()` numbering), `width` px wide, as a PNG data URL.
pub fn grab(index: u32, width: u32) -> Result<String, String> {
    let (px, dw, dh, _) = grab_raw(index, width)?;
    let png = png_rgb(&px, dw as u32, dh as u32).ok_or("The picture could not be encoded")?;
    Ok(format!("data:image/png;base64,{}", base64(&png)))
}

/// `grab` before encoding: (BGRA pixels, width, height, the monitor rectangle read).
pub fn grab_raw(index: u32, width: u32) -> Result<(Vec<u8>, i32, i32, (i32, i32, i32, i32)), String> {
    let mons = win::monitors();
    let m = mons.get(index as usize).ok_or("No such monitor")?;
    let rect = win::monitor_source(index).filter(|s| s.hmonitor != 0).and_then(|s| monitor_rect(s.hmonitor)).unwrap_or((m.x, m.y, m.width as i32, m.height as i32));
    let (dw, dh) = preview_size(rect.2, rect.3, width);
    let px = unsafe {
        let screen = GetDC(HWND::default());
        if screen.is_invalid() {
            return Err("The screen could not be read".into());
        }
        let px = stretch_from(screen, rect, dw, dh);
        ReleaseDC(HWND::default(), screen);
        px
    };
    let px = px.ok_or("The monitor's picture could not be copied")?;
    Ok((px, dw, dh, rect))
}

/// The monitor picker's live picture (ui/monpick.js). Fails soft: the picker then shows the
/// monitor's number and size only.
#[tauri::command(async)]
pub fn monitor_preview(index: u32, width: Option<u32>) -> Result<String, String> {
    grab(index, width.unwrap_or(320))
}

// ---------------------------------------------------------------------------
// `glint.exe --monprev-test <report.txt>`: nothing is read from the screen.

pub fn self_test(report_path: &str) {
    let mut lines: Vec<String> = Vec::new();
    let mut failed = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        if !ok {
            failed += 1;
        }
        lines.push(format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!("  [{detail}]") }));
    };
    check("size: a 16:9 monitor at 320 is 320x180", preview_size(2560, 1440, 320) == (320, 180), format!("{:?}", preview_size(2560, 1440, 320)));
    check("size: a portrait monitor keeps its shape", preview_size(1080, 1920, 320) == (320, 569), format!("{:?}", preview_size(1080, 1920, 320)));
    check("size: an ultrawide and a 16:10 monitor", preview_size(3440, 1440, 320) == (320, 134) && preview_size(1680, 1050, 320) == (320, 200), format!("{:?} {:?}", preview_size(3440, 1440, 320), preview_size(1680, 1050, 320)));
    check("size: the width asked is kept within 64..640, a zero monitor does not divide by zero", preview_size(1920, 1080, 5000).0 == 640 && preview_size(1920, 1080, 1).0 == 64 && preview_size(0, 0, 320) == (320, 180), String::new());
    check("base64: known vectors", base64(b"") == "" && base64(b"f") == "Zg==" && base64(b"fo") == "Zm8=" && base64(b"foo") == "Zm9v" && base64(b"foobar") == "Zm9vYmFy", String::new());
    // A synthetic 1280x720 picture in memory: left half red, right half blue, a green bar at the top.
    let (sw, sh) = (1280, 720);
    let scaled = unsafe {
        dib(sw, sh).and_then(|(dc, bmp, bits, old)| {
            let px = std::slice::from_raw_parts_mut(bits, (sw * sh * 4) as usize);
            for y in 0..sh {
                for x in 0..sw {
                    let i = ((y * sw + x) * 4) as usize;
                    let (r, g, b) = if y < 72 { (0, 255, 0) } else if x < sw / 2 { (255, 0, 0) } else { (0, 0, 255) };
                    px[i] = b;
                    px[i + 1] = g;
                    px[i + 2] = r;
                    px[i + 3] = 0;
                }
            }
            let (dw, dh) = preview_size(sw, sh, 320);
            let out = stretch_from(dc, (0, 0, sw, sh), dw, dh);
            free(dc, bmp, old);
            out.map(|o| (o, dw, dh))
        })
    };
    match scaled {
        Some((px, dw, dh)) => {
            let at = |x: i32, y: i32| {
                let i = ((y * dw + x) * 4) as usize;
                (px[i + 2], px[i + 1], px[i])
            };
            let near = |a: (u8, u8, u8), b: (u8, u8, u8)| (a.0 as i32 - b.0 as i32).abs() < 40 && (a.1 as i32 - b.1 as i32).abs() < 40 && (a.2 as i32 - b.2 as i32).abs() < 40;
            check("scale: a synthetic bitmap is scaled by GDI to 320x180", (dw, dh) == (320, 180) && px.len() == (320 * 180 * 4) as usize, format!("{dw}x{dh}"));
            check("scale: its colors land where they were (red left, blue right, green on top)", near(at(40, 120), (255, 0, 0)) && near(at(280, 120), (0, 0, 255)) && near(at(160, 5), (0, 255, 0)), format!("{:?} {:?} {:?}", at(40, 120), at(280, 120), at(160, 5)));
            match png_rgb(&px, dw as u32, dh as u32) {
                Some(png) => {
                    let url = format!("data:image/png;base64,{}", base64(&png));
                    let dec = png::Decoder::new(std::io::Cursor::new(png.clone()));
                    let info = dec.read_info().ok().map(|r| (r.info().width, r.info().height));
                    check("encode: a PNG of the same size, as a data URL", info == Some((320, 180)) && url.starts_with("data:image/png;base64,iVBOR"), format!("{info:?}, {} bytes", png.len()));
                    check("encode: small enough to send once a second (under 200 KB)", url.len() < 200_000, format!("{} bytes", url.len()));
                }
                None => check("encode: a PNG", false, String::new()),
            }
        }
        None => check("scale: a synthetic bitmap is scaled by GDI", false, "no DIB".into()),
    }
    check("encode: too few pixels are refused", png_rgb(&[0; 10], 4, 4).is_none(), String::new());
    let n = lines.len();
    lines.push(format!("{n} checks, {failed} failed"));
    let _ = std::fs::write(report_path, lines.join("\n") + "\n");
}
